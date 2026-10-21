//! The plugin's commands.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand, SimplePluginCommand};
use nu_protocol::{
    Category, Example, HandlerGuard, LabeledError, ListStream, PipelineData, Record, SignalAction,
    Signals, Signature, Span, Spanned, SyntaxShape, Type, Value,
};
use stardust::{Commit, Database, Document, Format, Options, Rows, RunOptions, Source};

use crate::convert::{from_document, from_sd, sd_err, to_document};
use crate::plugin::{DbHandle, StardustPlugin};
use crate::pool;

pub fn all() -> Vec<Box<dyn PluginCommand<Plugin = StardustPlugin>>> {
    let mut commands: Vec<Box<dyn PluginCommand<Plugin = StardustPlugin>>> = vec![
        Box::new(Main),
        Box::new(Open),
        Box::new(Close),
        Box::new(Patch),
        Box::new(Query),
        Box::new(Mutate),
        Box::new(Subscribe),
        Box::new(Define),
        Box::new(Definition),
        Box::new(Definitions),
        Box::new(Eval),
        Box::new(License),
        Box::new(Plugins),
        Box::new(PluginCall),
    ];
    commands.extend(formats());
    commands.into_iter().map(|command| Box::new(Pooled(command)) as Box<dyn PluginCommand<Plugin = StardustPlugin>>).collect()
}

/// A command that runs on the Stardust pool (see `pool`).
struct Pooled(Box<dyn PluginCommand<Plugin = StardustPlugin>>);

impl PluginCommand for Pooled {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        self.0.name()
    }
    fn signature(&self) -> Signature {
        self.0.signature()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn extra_description(&self) -> &str {
        self.0.extra_description()
    }
    fn search_terms(&self) -> Vec<&str> {
        self.0.search_terms()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        self.0.examples()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: PipelineData) -> Result<PipelineData, LabeledError> {
        pool::run(|| self.0.run(plugin, engine, call, input))
    }
}

/// Declares a command struct with its signature builder.
macro_rules! simple {
    ($ty:ident, $name:literal, $sig:expr) => {
        pub struct $ty;
        impl $ty {
            fn sig() -> Signature {
                $sig(Signature::build($name)).category(Category::Database)
            }
        }
    };
}

fn db_arg(sig: Signature) -> Signature {
    sig.required("db", SyntaxShape::Any, "A database from `stardust open`.")
}

/// Adds `--format`, which says how string documents are written.
fn format_flag(sig: Signature) -> Signature {
    sig.named(
        "format",
        SyntaxShape::String,
        "How string documents are written: dust (default), json or edn. Binary is TRON.",
        Some('f'),
    )
}

/// The format of string documents, from `--format`.
fn format(call: &EvaluatedCall) -> Result<Format, LabeledError> {
    let Some(flag) = call.get_flag::<Spanned<String>>("format")? else {
        return Ok(Format::DUST);
    };
    match flag.item.as_str() {
        "dust" => Ok(Format::DUST),
        "json" => Ok(Format::JSON),
        "edn" => Ok(Format::EDN),
        _ => Err(LabeledError::new("unknown format").with_label("expected dust, json or edn", flag.span)),
    }
}

/// A document from a nu value: a string is text in `format`, binary is TRON,
/// and anything else is nu data.
fn document(value: &Value, format: Format) -> Result<Document, LabeledError> {
    let span = value.span();
    match value {
        Value::String { val, .. } => stardust::parse(val, format).map_err(|e| sd_err(e, span)),
        Value::Binary { val, .. } => stardust::parse(val, Format::TRON).map_err(|e| sd_err(e, span)),
        other => to_document(other),
    }
}

/// A definition source: a stored path or an inline definition.
enum Src {
    Path(String),
    Inline(Document),
}

impl Src {
    /// A string is a document too, so a stored path is a document that holds
    /// only a string: `people/by_age` in DUST, `"people/by_age"` in JSON.
    fn arg(call: &EvaluatedCall, index: usize) -> Result<Src, LabeledError> {
        let value = call.req::<Value>(index)?;
        let doc = document(&value, format(call)?)?;
        let root = doc.root().and_then(|id| doc.read(id)).map_err(|e| sd_err(e, value.span()))?;
        if root.kind == stardust::Kind::STRING {
            return Ok(Src::Path(root.text.to_string()));
        }
        Ok(Src::Inline(doc))
    }

    fn get(&self) -> Source<'_> {
        match self {
            Src::Path(p) => Source::Path(p),
            Src::Inline(d) => Source::Document(d),
        }
    }
}

fn params(call: &EvaluatedCall) -> Result<Option<Document>, LabeledError> {
    let format = format(call)?;
    call.get_flag::<Value>("params")?.map(|v| document(&v, format)).transpose()
}

fn run_options(call: &EvaluatedCall) -> Result<RunOptions, LabeledError> {
    let mut options = RunOptions::default();
    if let Some(n) = call.get_flag::<i64>("page-size")? {
        options.page_size = u32::try_from(n).map_err(|_| {
            LabeledError::new("invalid page size").with_label("must be 0 or more", call.head)
        })?;
    }
    if let Some(tx) = call.get_flag::<i64>("as-of")? {
        options.as_of = tx;
    }
    if let Some(Value::Duration { val, .. }) = call.get_flag::<Value>("max-cost")? {
        options.max_cost = Duration::from_nanos(val.max(0) as u64);
    }
    Ok(options)
}

fn with_run_flags(sig: Signature) -> Signature {
    format_flag(sig).named("params", SyntaxShape::Any, "Parameters, merged over stored defaults.", Some('p'))
        .named("as-of", SyntaxShape::Int, "Run a stored definition as of this transaction.", None)
        .named("max-cost", SyntaxShape::Duration, "CPU time limit for the run.", None)
}

fn commit_value(commit: &Commit, span: Span) -> Value {
    let mut ids = Record::new();
    let mut sorted: Vec<_> = commit.ids.iter().collect();
    sorted.sort_by_key(|(_, id)| **id);
    for (name, id) in sorted {
        ids.push(name.clone(), Value::int(*id, span));
    }
    let committed_at = match commit.committed_at {
        Some(t) => Value::date(chrono::DateTime::<chrono::Utc>::from(t).fixed_offset(), span),
        None => Value::nothing(span),
    };
    let mut record = Record::new();
    record.push("status", Value::string(commit.status.name(), span));
    record.push("transaction", Value::int(commit.transaction, span));
    record.push("facts", Value::int(commit.facts, span));
    record.push("committed_at", committed_at);
    record.push("ids", Value::record(ids, span));
    Value::record(record, span)
}

// ---------------------------------------------------------------- stardust

simple!(Main, "stardust", |s: Signature| s);
impl SimplePluginCommand for Main {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust"
    }
    fn description(&self) -> &str {
        "Embedded Stardust databases. Start with `stardust open`."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, _: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        Ok(Value::string(engine.get_help()?, call.head))
    }
}

// ---------------------------------------------------------------- open / close

simple!(Open, "stardust open", |s: Signature| {
    s.input_output_type(Type::Nothing, Type::Custom("stardust-db".into()))
        .required("path", SyntaxShape::Filepath, "The database file.")
        .switch("no-create", "Fail if the file does not exist.", None)
        .named("lock-timeout", SyntaxShape::Duration, "Wait this long for another process's lock (default: fail at once).", None)
        .named("plugins", SyntaxShape::Record(vec![].into()), "Stardust plugins to enable, with their settings (default: none).", None)
});
impl SimplePluginCommand for Open {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust open"
    }
    fn description(&self) -> &str {
        "Open (or create) a Stardust database and return a handle to it."
    }
    fn extra_description(&self) -> &str {
        "The database stays open while any copy of the handle exists, or until `stardust close`."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example { example: "let db = stardust open people.db", description: "Open a database", result: None }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let path: String = call.req(0)?;
        let path = std::path::Path::new(&engine.get_current_dir()?).join(path);
        let path = path.to_string_lossy().into_owned();
        let mut options = Options { create: !call.has_flag("no-create")?, ..Options::default() };
        if let Some(Value::Duration { val, .. }) = call.get_flag::<Value>("lock-timeout")? {
            options.lock_timeout = Some(Duration::from_nanos(val.max(0) as u64));
        }
        if let Some(Value::Record { val, .. }) = call.get_flag::<Value>("plugins")? {
            let mut plugins = std::collections::BTreeMap::new();
            for (name, settings) in val.iter() {
                plugins.insert(name.clone(), crate::convert::to_sd(settings)?);
            }
            options.plugins = Some(plugins);
        }
        let db = stardust::open(&path, &options).map_err(|e| sd_err(e, call.head))?;
        let id = plugin.register(engine, db)?;
        Ok(Value::custom(Box::new(DbHandle { id, path }), call.head))
    }
}

simple!(Close, "stardust close", |s: Signature| db_arg(s.input_output_type(Type::Nothing, Type::Nothing)));
impl SimplePluginCommand for Close {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust close"
    }
    fn description(&self) -> &str {
        "Close a database now, without waiting for its handle to be dropped."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let value: Value = call.req(0)?;
        plugin.close(engine, DbHandle::from_value(&value)?.id)?;
        Ok(Value::nothing(call.head))
    }
}

// ---------------------------------------------------------------- writes

simple!(Patch, "stardust patch", |s: Signature| {
    format_flag(db_arg(s.input_output_types(vec![
        (Type::record(), Type::record()),
        (Type::String, Type::record()),
        (Type::Binary, Type::record()),
    ])))
});
impl SimplePluginCommand for Patch {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust patch"
    }
    fn description(&self) -> &str {
        "Commit an entity-keyed merge patch as one transaction."
    }
    fn extra_description(&self) -> &str {
        "The patch is a record, DUST text (or the --format given), or TRON binary. Keys like `#_ada` name new entities; the result's `ids` maps them to entity IDs."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example {
                example: "'{#_ada {name Ada role engineer age 36}}' | stardust patch $db",
                description: "Create a person, as DUST",
                result: None,
            },
            Example {
                example: "{'#_ada': {name: Ada role: engineer age: 36}} | stardust patch $db",
                description: "The same, as a record: quote #_ keys, since # starts a nu comment",
                result: None,
            },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let patch = document(input, format(call)?)?;
        let commit = db.merge_patch(&patch, None).map_err(|e| sd_err(e, input.span()))?;
        Ok(commit_value(&commit, call.head))
    }
}

simple!(Mutate, "stardust mutate", |s: Signature| {
    with_run_flags(db_arg(s.input_output_type(Type::Nothing, Type::record())))
        .required("mutation", SyntaxShape::Any, "An inline definition (a record, or DUST text), or a stored mutation path.")
});
impl SimplePluginCommand for Mutate {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust mutate"
    }
    fn description(&self) -> &str {
        "Run a mutation and return its commit."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let src = Src::arg(call, 1)?;
        let params = params(call)?;
        let mutation = db.mutation(src.get()).map_err(|e| sd_err(e, call.head))?;
        let commit = mutation
            .run(params.as_ref(), &run_options(call)?, None)
            .map_err(|e| sd_err(e, call.head))?;
        Ok(commit_value(&commit, call.head))
    }
}

// ---------------------------------------------------------------- query

pub struct Query;
impl PluginCommand for Query {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust query"
    }
    fn description(&self) -> &str {
        "Run a query and stream its rows."
    }
    fn extra_description(&self) -> &str {
        "Rows arrive one page at a time, so `| first 10` stops reading early."
    }
    fn signature(&self) -> Signature {
        with_run_flags(db_arg(Signature::build(self.name())))
            .input_output_type(Type::Nothing, Type::list(Type::Any))
            .required("query", SyntaxShape::Any, "An inline definition (a record, or DUST text), or a stored query path.")
            .named("page-size", SyntaxShape::Int, "Rows per page (0: the engine default).", None)
            .category(Category::Database)
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example { example: "stardust query $db people/by_age --params {min: 30}", description: "Run a stored query", result: None },
            Example {
                example: "stardust query $db {find: [?name] where: [[?e name ?name]]}",
                description: "Run an inline query",
                result: None,
            },
            Example {
                example: "stardust query $db '{find [?name] where [[?e name ?name]]}'",
                description: "The same, as DUST",
                result: None,
            },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: PipelineData) -> Result<PipelineData, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let src = Src::arg(call, 1)?;
        let params = params(call)?;
        let query = db.query(src.get()).map_err(|e| sd_err(e, call.head))?;
        let rows = query
            .run(params.as_ref(), &run_options(call)?, None)
            .map_err(|e| sd_err(e, call.head))?;
        let stream = RowStream { _db: db, rows: Some(rows), page: Vec::new().into_iter(), span: call.head };
        Ok(PipelineData::list_stream(
            ListStream::new(stream, call.head, engine.signals().clone()),
            None,
        ))
    }
}

/// Reads pages lazily. Holds the database so it stays open until the
/// stream ends, even if its handle is dropped first.
struct RowStream {
    _db: Arc<Database>,
    rows: Option<Rows>,
    page: std::vec::IntoIter<Value>,
    span: Span,
}

impl Drop for RowStream {
    fn drop(&mut self) {
        pool::drop(self.rows.take());
    }
}

impl Iterator for RowStream {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        loop {
            if let Some(row) = self.page.next() {
                return Some(row);
            }
            let rows = self.rows.as_mut()?;
            let page = match pool::run(|| rows.next_page().and_then(|p| p.to_value())) {
                Ok(page) => page,
                Err(e) => {
                    pool::drop(self.rows.take());
                    return Some(Value::error(sd_err(e, self.span).into(), self.span));
                }
            };
            match from_sd(page, self.span) {
                Value::List { vals, .. } if !vals.is_empty() => self.page = vals.into_iter(),
                _ => {
                    pool::drop(self.rows.take()); // the empty page ends the cursor
                    return None;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- subscribe

pub struct Subscribe;
impl PluginCommand for Subscribe {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust subscribe"
    }
    fn description(&self) -> &str {
        "Subscribe to a query: stream its result now, and again after each commit that changes it."
    }
    fn extra_description(&self) -> &str {
        "Runs until interrupted with Ctrl-C, or until the database closes."
    }
    fn signature(&self) -> Signature {
        with_run_flags(db_arg(Signature::build(self.name())))
            .input_output_type(Type::Nothing, Type::list(Type::record()))
            .required("query", SyntaxShape::Any, "An inline definition (a record, or DUST text), or a stored query path.")
            .category(Category::Database)
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: PipelineData) -> Result<PipelineData, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let src = Src::arg(call, 1)?;
        let params = params(call)?;
        let query = db.query(src.get()).map_err(|e| sd_err(e, call.head))?;
        let subscription = query
            .subscribe(params.as_ref(), &run_options(call)?)
            .map_err(|e| sd_err(e, call.head))?;
        // Ctrl-C cancels from nu's signal thread, which wakes a waiting `next`
        // at once. The guard unregisters the handler when the stream drops.
        let canceller = subscription.canceller();
        let cancel = canceller.clone();
        let guard = engine.register_signal_handler(Box::new(move |action| {
            if let SignalAction::Interrupt = action {
                cancel.cancel();
            }
        }))?;
        // An interrupt that landed before the handler existed.
        if engine.signals().interrupted() {
            canceller.cancel();
        }
        let events = EventStream {
            _db: db,
            _guard: guard,
            sub: Some(subscription),
            engine: engine.clone(),
            span: call.head,
        };
        // Our own handler ends the stream, so ListStream needs no signals.
        Ok(PipelineData::list_stream(ListStream::new(events, call.head, Signals::empty()), None))
    }
}

/// How long `next` waits before checking that nu is still there.
const LIVENESS: Duration = Duration::from_secs(1);

/// Ctrl-C reaches the stream through a signal handler, but nothing tells a
/// waiting iterator that nu dropped the stream (after `| first n`) or exited,
/// and the plugin cannot exit while this thread runs. So between events it
/// pings nu, and ends when that fails. Dropping the subscription cancels it.
struct EventStream {
    _db: Arc<Database>,
    _guard: HandlerGuard,
    sub: Option<stardust::Subscription>,
    engine: EngineInterface,
    span: Span,
}

impl Drop for EventStream {
    fn drop(&mut self) {
        pool::drop(self.sub.take());
    }
}

impl Iterator for EventStream {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        let span = self.span;
        loop {
            let sub = self.sub.as_mut()?;
            // Dropping a pending `next` future loses no event, so each wait
            // can give up after LIVENESS and start a fresh one.
            match wait(sub.next(), LIVENESS) {
                None => {
                    if self.engine.get_current_dir().is_err() {
                        pool::drop(self.sub.take());
                        return None;
                    }
                }
                Some(Ok(Some(event))) => {
                    // Reading the document calls Stardust, and so does
                    // releasing it: both on the pool.
                    let (result, event) = pool::run(move || {
                        let result = match from_document(&event.result, span) {
                            Ok(v) => v,
                            Err(e) => Value::error(e.into(), span),
                        };
                        (result, (event.sequence, event.transaction, event.superseded))
                    });
                    let (sequence, transaction, superseded) = event;
                    let mut record = Record::new();
                    record.push("sequence", Value::int(sequence, span));
                    record.push("transaction", Value::int(transaction, span));
                    record.push("superseded", Value::int(superseded, span));
                    record.push("result", result);
                    return Some(Value::record(record, span));
                }
                // Cancelled, or the database closed.
                Some(Ok(None)) => {
                    pool::drop(self.sub.take());
                    return None;
                }
                Some(Err(e)) => {
                    pool::drop(self.sub.take());
                    return Some(Value::error(sd_err(e, span).into(), span));
                }
            }
        }
    }
}

/// Polls `future` for up to `timeout`, parking this thread between polls.
/// The binding's subscriptions are async only; nu's streams are iterators.
/// Each poll calls Stardust, so it runs on the pool; the wait doesn't.
fn wait<F: Future + Send>(future: F, timeout: Duration) -> Option<F::Output>
where
    F::Output: Send,
{
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut future = pin!(future);
    let deadline = Instant::now() + timeout;
    loop {
        if let Poll::Ready(output) = pool::run(|| future.as_mut().poll(&mut Context::from_waker(&waker))) {
            return Some(output);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        // Wakes early on unpark; a spurious wake just polls again.
        std::thread::park_timeout(left);
    }
}

// ---------------------------------------------------------------- definitions

simple!(Define, "stardust define", |s: Signature| {
    format_flag(db_arg(s.input_output_type(Type::Nothing, Type::record())))
        .required("kind", SyntaxShape::String, "queries, mutations, schemas or expressions.")
        .required("path", SyntaxShape::String, "The definition's path, e.g. people/by_age.")
        .required("definition", SyntaxShape::Any, "A merge patch onto the definition, as a record or DUST text; null deletes it.")
});
impl SimplePluginCommand for Define {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust define"
    }
    fn description(&self) -> &str {
        "Store, update or delete a definition."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "stardust define $db queries people/by_age '{find [?name] parameters {min 18} where [[?e name ?name] [?e age ?age] [>= ?age ?min]]}'",
            description: "Store a query with a default parameter",
            result: None,
        }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let kind: String = call.req(1)?;
        let path: String = call.req(2)?;
        let patch = document(&call.req::<Value>(3)?, format(call)?)?;
        let commit = db.patch_definition(&kind, &path, &patch).map_err(|e| sd_err(e, call.head))?;
        Ok(commit_value(&commit, call.head))
    }
}

simple!(Definition, "stardust definition", |s: Signature| {
    db_arg(s.input_output_type(Type::Nothing, Type::Any))
        .required("kind", SyntaxShape::String, "queries, mutations, schemas or expressions.")
        .required("path", SyntaxShape::String, "The definition's path.")
        .named("as-of", SyntaxShape::Int, "The version as of this transaction (default: latest).", None)
});
impl SimplePluginCommand for Definition {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust definition"
    }
    fn description(&self) -> &str {
        "Read a stored definition."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let kind: String = call.req(1)?;
        let path: String = call.req(2)?;
        let as_of = call.get_flag::<i64>("as-of")?.unwrap_or(0);
        let doc = db.definition(&kind, &path, as_of).map_err(|e| sd_err(e, call.head))?;
        from_document(&doc, call.head)
    }
}

simple!(Definitions, "stardust definitions", |s: Signature| {
    db_arg(s.input_output_type(Type::Nothing, Type::list(Type::String)))
        .required("kind", SyntaxShape::String, "queries, mutations, schemas or expressions.")
        .optional("pattern", SyntaxShape::String, "A regular expression over paths (default: all).")
});
impl SimplePluginCommand for Definitions {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust definitions"
    }
    fn description(&self) -> &str {
        "List stored definition paths of one kind."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let kind: String = call.req(1)?;
        let pattern: String = call.opt(2)?.unwrap_or_default();
        let doc = db.definitions(&kind, &pattern).map_err(|e| sd_err(e, call.head))?;
        from_document(&doc, call.head)
    }
}

// ---------------------------------------------------------------- expressions

simple!(Eval, "stardust eval", |s: Signature| {
    format_flag(s.input_output_type(Type::Nothing, Type::Any))
        .required("expression", SyntaxShape::Any, "An expression as a list, or as DUST text.")
        .named("input", SyntaxShape::Any, "Values for the expression's ?variables.", Some('i'))
});
impl SimplePluginCommand for Eval {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust eval"
    }
    fn description(&self) -> &str {
        "Build and evaluate a Stardust expression. No database needed."
    }
    fn extra_description(&self) -> &str {
        "A string is parsed as DUST text (or the --format given); any other value is the expression itself."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example { example: "stardust eval '[* ?n 2]' --input {n: 21}", description: "DUST text", result: None },
            Example { example: "stardust eval [* ?n 2] --input {n: 21}", description: "A nu list", result: None },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let expr: Value = call.req(0)?;
        let span = expr.span();
        let format = format(call)?;
        let expression = document(&expr, format)?;
        let input = call.get_flag::<Value>("input")?.map(|v| document(&v, format)).transpose()?;
        let program = stardust::build(&expression, None).map_err(|e| sd_err(e, span))?;
        let result = program.evaluate(input.as_ref(), None).map_err(|e| sd_err(e, span))?;
        from_document(&result, call.head)
    }
}

// ---------------------------------------------------------------- license

simple!(License, "stardust license", |s: Signature| s.input_output_type(Type::Nothing, Type::record()));
impl SimplePluginCommand for License {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust license"
    }
    fn description(&self) -> &str {
        "Activate the license if needed, and show what it covers. Never shows the token."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let span = call.head;
        let activation = plugin.ensure_licensed(engine, span).err();
        let info = stardust::license().map_err(|e| sd_err(e, span))?;
        let time = |t: Option<std::time::SystemTime>| match t {
            Some(t) => Value::date(chrono::DateTime::<chrono::Utc>::from(t).fixed_offset(), span),
            None => Value::nothing(span),
        };
        let mut record = Record::new();
        record.push("active", Value::bool(info.active, span));
        if let Some(e) = activation {
            record.push("error", Value::string(e.to_string(), span));
        }
        record.push("failure", Value::string(info.failure, span));
        record.push("name", Value::string(info.name, span));
        record.push("license", Value::string(info.license, span));
        record.push("subject", Value::string(info.subject, span));
        record.push("license_id", Value::string(info.license_id, span));
        record.push("support_end", time(info.support_end));
        record.push("expires", time(info.expires));
        record.push("build", time(info.build));
        Ok(Value::record(record, span))
    }
}

// ---------------------------------------------------------------- plugins

simple!(Plugins, "stardust plugins", |s: Signature| s.input_output_type(Type::Nothing, Type::table()));
impl SimplePluginCommand for Plugins {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust plugins"
    }
    fn description(&self) -> &str {
        "List the Stardust plugins this build can load."
    }
    fn extra_description(&self) -> &str {
        "Enable them by name with `stardust open --plugins {name: {}}`. A plugin library that cannot be used is listed with `unavailable`, the reason."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example { example: "stardust plugins | select name description", description: "What each plugin does", result: None }]
    }
    fn run(&self, _: &StardustPlugin, _: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        // Plugin inspection needs no license.
        let doc = stardust::plugins().map_err(|e| sd_err(e, call.head))?;
        from_document(&doc, call.head)
    }
}

simple!(PluginCall, "stardust plugin-call", |s: Signature| {
    format_flag(db_arg(s.input_output_type(Type::Nothing, Type::Any)))
        .required("plugin", SyntaxShape::String, "An enabled plugin, e.g. reactors.")
        .required("operation", SyntaxShape::String, "The operation, e.g. status or sync.")
        .optional("request", SyntaxShape::Any, "The operation's request, if it takes one.")
});
impl SimplePluginCommand for PluginCall {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust plugin-call"
    }
    fn description(&self) -> &str {
        "Call an operation of a plugin the database enables."
    }
    fn extra_description(&self) -> &str {
        "Plugins conventionally have status, and sync to wait for committed work. sync takes an optional {maxWait: <duration>} and checks once without it. Stream operations are not supported."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example {
                example: "let db = stardust open people.db --plugins {reactors: {}}; stardust plugin-call $db reactors status",
                description: "The reactor runtime's status",
                result: None,
            },
            Example {
                example: "stardust plugin-call $db reactors sync {maxWait: 5sec}",
                description: "Wait up to 5 seconds for live mutations to catch up",
                result: None,
            },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, _: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let db = plugin.db(call, 0)?;
        let name: String = call.req(1)?;
        let operation: String = call.req(2)?;
        let format = format(call)?;
        let request = call.opt::<Value>(3)?.map(|v| document(&v, format)).transpose()?;
        let result = db
            .plugin_call(&name, &operation, request.as_ref())
            .map_err(|e| sd_err(e, call.head))?;
        from_document(&result, call.head)
    }
}

// ---------------------------------------------------------------- formats

/// `from <format>` for the formats Stardust parses. `open file.<ext>` uses it.
pub struct FromFormat {
    name: &'static str,
    format: Format,
    label: &'static str,
}

impl SimplePluginCommand for FromFormat {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        match self.label {
            "DUST" => "Convert DUST text into structured data.",
            "EDN" => "Convert EDN text into structured data.",
            _ => "Convert TRON data into structured data.",
        }
    }
    fn signature(&self) -> Signature {
        let input = if self.format == Format::TRON { Type::Binary } else { Type::String };
        Signature::build(self.name).input_output_type(input, Type::Any).category(Category::Formats)
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let span = input.span();
        let doc = match input {
            Value::String { val, .. } => stardust::parse(val, self.format),
            Value::Binary { val, .. } => stardust::parse(val, self.format),
            other => {
                return Err(LabeledError::new(format!("expected {} input", if self.format == Format::TRON { "binary" } else { "string" }))
                    .with_label(format!("got {}", other.get_type()), span));
            }
        }
        .map_err(|e| sd_err(e, span))?;
        from_document(&doc, call.head)
    }
}

/// `to <format>` for the formats Stardust writes.
pub struct ToFormat {
    name: &'static str,
    format: Format,
    label: &'static str,
}

impl SimplePluginCommand for ToFormat {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        match self.label {
            "DUST" => "Convert Nu values into DUST text.",
            "EDN" => "Convert Nu values into EDN text.",
            _ => "Convert Nu values into TRON.",
        }
    }
    fn signature(&self) -> Signature {
        let sig = Signature::build(self.name).category(Category::Formats);
        if self.format == Format::TRON {
            sig.input_output_type(Type::Any, Type::Binary)
        } else {
            sig.input_output_type(Type::Any, Type::String)
                .named("width", SyntaxShape::Int, "Wrap lines at this width (0: the default).", Some('w'))
        }
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let width = if self.format == Format::TRON { 0 } else { call.get_flag::<i64>("width")?.unwrap_or(0) };
        let bytes = to_document(input)?
            .serialize(self.format, width)
            .map_err(|e| sd_err(e, call.head))?;
        if self.format == Format::TRON {
            return Ok(Value::binary(bytes, call.head));
        }
        let text = String::from_utf8(bytes)
            .map_err(|e| LabeledError::new("invalid UTF-8 from serializer").with_label(e.to_string(), call.head))?;
        Ok(Value::string(text, call.head))
    }
}

fn formats() -> Vec<Box<dyn PluginCommand<Plugin = StardustPlugin>>> {
    let mut commands: Vec<Box<dyn PluginCommand<Plugin = StardustPlugin>>> = Vec::new();
    for (from, to, format, label) in [
        ("from dust", "to dust", Format::DUST, "DUST"),
        ("from edn", "to edn", Format::EDN, "EDN"),
        ("from tron", "to tron", Format::TRON, "TRON"),
    ] {
        commands.push(Box::new(FromFormat { name: from, format, label }));
        commands.push(Box::new(ToFormat { name: to, format, label }));
    }
    commands
}
