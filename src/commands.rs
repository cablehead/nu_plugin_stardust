//! The plugin's commands.

use std::sync::Arc;
use std::time::Duration;

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand, SimplePluginCommand};
use nu_protocol::{
    Category, Example, HandlerGuard, LabeledError, ListStream, PipelineData, Record, SignalAction,
    Signals, Signature, Span, SyntaxShape, Type, Value,
};
use stardust::{Commit, Database, Document, Format, Options, Rows, RunOptions, Source, Status};

use crate::convert::{from_document, from_sd, sd_err, to_document};
use crate::plugin::{DbHandle, StardustPlugin};

pub fn all() -> Vec<Box<dyn PluginCommand<Plugin = StardustPlugin>>> {
    vec![
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
        Box::new(FromDust),
        Box::new(ToDust),
    ]
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

/// A definition source: a stored path (string) or an inline definition (record).
enum Src {
    Path(String),
    Inline(Document),
}

impl Src {
    fn arg(call: &EvaluatedCall, index: usize) -> Result<Src, LabeledError> {
        match call.req::<Value>(index)? {
            Value::String { val, .. } => Ok(Src::Path(val)),
            other => Ok(Src::Inline(to_document(&other)?)),
        }
    }

    fn get(&self) -> Source<'_> {
        match self {
            Src::Path(p) => Source::Path(p),
            Src::Inline(d) => Source::Document(d),
        }
    }
}

fn params(call: &EvaluatedCall) -> Result<Option<Document>, LabeledError> {
    call.get_flag::<Value>("params")?.map(|v| to_document(&v)).transpose()
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
    sig.named("params", SyntaxShape::Record(vec![].into()), "Parameters, merged over stored defaults.", Some('p'))
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
        .named("plugins", SyntaxShape::Record(vec![].into()), "Stardust plugins to enable, with their settings (default: all).", None)
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
    db_arg(s.input_output_type(Type::record(), Type::record()))
});
impl SimplePluginCommand for Patch {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust patch"
    }
    fn description(&self) -> &str {
        "Commit an entity-keyed merge patch from the input record as one transaction."
    }
    fn extra_description(&self) -> &str {
        "Keys like `#_ada` name new entities; the result's `ids` maps them to entity IDs."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "{'#_ada': {name: Ada role: engineer age: 36}} | stardust patch $db",
            description: "Create a person",
            result: None,
        }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let patch = to_document(input)?;
        let commit = db.merge_patch(&patch, None).map_err(|e| sd_err(e, input.span()))?;
        Ok(commit_value(&commit, call.head))
    }
}

simple!(Mutate, "stardust mutate", |s: Signature| {
    with_run_flags(db_arg(s.input_output_type(Type::Nothing, Type::record())))
        .required("mutation", SyntaxShape::Any, "A stored mutation path, or an inline definition record.")
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
            .required("query", SyntaxShape::Any, "A stored query path, or an inline definition record.")
            .named("page-size", SyntaxShape::Int, "Rows per page (0: the engine default).", None)
            .category(Category::Database)
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example { example: "stardust query $db people/by_age --params {min: 30}", description: "Run a stored query", result: None },
            Example {
                example: "stardust query $db {find: ['?name'] where: [['?e' name '?name']]}",
                description: "Run an inline query",
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

impl Iterator for RowStream {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        loop {
            if let Some(row) = self.page.next() {
                return Some(row);
            }
            let rows = self.rows.as_mut()?;
            let page = match rows.next_page().and_then(|p| p.to_value()) {
                Ok(page) => page,
                Err(e) => {
                    self.rows = None;
                    return Some(Value::error(sd_err(e, self.span).into(), self.span));
                }
            };
            match from_sd(page, self.span) {
                Value::List { vals, .. } if !vals.is_empty() => self.page = vals.into_iter(),
                _ => {
                    self.rows = None; // the empty page ends the cursor
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
            .required("query", SyntaxShape::Any, "A stored query path, or an inline definition record.")
            .category(Category::Database)
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: PipelineData) -> Result<PipelineData, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let src = Src::arg(call, 1)?;
        let params = params(call)?;
        let query = db.query(src.get()).map_err(|e| sd_err(e, call.head))?;
        let subscription = Arc::new(
            query
                .subscribe(params.as_ref(), &run_options(call)?)
                .map_err(|e| sd_err(e, call.head))?,
        );
        // Ctrl-C cancels from nu's signal thread, which wakes a blocked `next`
        // at once. The guard unregisters the handler when the stream drops.
        let cancel = Arc::clone(&subscription);
        let guard = engine.register_signal_handler(Box::new(move |action| {
            if let SignalAction::Interrupt = action {
                cancel.cancel();
            }
        }))?;
        // An interrupt that landed before the handler existed.
        if engine.signals().interrupted() {
            subscription.cancel();
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
/// blocked iterator that nu dropped the stream (after `| first n`) or exited,
/// and the plugin cannot exit while this thread runs. So between events it
/// pings nu, and ends when that fails.
struct EventStream {
    _db: Arc<Database>,
    _guard: HandlerGuard,
    sub: Option<Arc<stardust::Subscription>>,
    engine: EngineInterface,
    span: Span,
}

impl Iterator for EventStream {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        let span = self.span;
        loop {
            let sub = self.sub.as_ref()?;
            match sub.next(LIVENESS) {
                Ok(None) => {
                    if self.engine.get_current_dir().is_err() {
                        sub.cancel();
                        self.sub = None;
                        return None;
                    }
                }
                Ok(Some(event)) => {
                    let result = match from_document(&event.result, span) {
                        Ok(v) => v,
                        Err(e) => Value::error(e.into(), span),
                    };
                    let mut record = Record::new();
                    record.push("sequence", Value::int(event.sequence, span));
                    record.push("transaction", Value::int(event.transaction, span));
                    record.push("superseded", Value::int(event.superseded, span));
                    record.push("result", result);
                    return Some(Value::record(record, span));
                }
                Err(e) => {
                    self.sub = None;
                    if e.status == Status::CANCELLED || e.status == Status::CLOSED {
                        return None;
                    }
                    return Some(Value::error(sd_err(e, span).into(), span));
                }
            }
        }
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Some(sub) = self.sub.take() {
            sub.cancel();
        }
    }
}

// ---------------------------------------------------------------- definitions

simple!(Define, "stardust define", |s: Signature| {
    db_arg(s.input_output_type(Type::Nothing, Type::record()))
        .required("kind", SyntaxShape::String, "queries, mutations, schemas or expressions.")
        .required("path", SyntaxShape::String, "The definition's path, e.g. people/by_age.")
        .required("definition", SyntaxShape::Any, "A merge patch onto the definition; null deletes it.")
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
            example: "stardust define $db queries people/by_age {find: ['?name'] parameters: {min: 18} where: [['?e' name '?name'] ['?e' age '?age'] ['>=' '?age' '?min']]}",
            description: "Store a query with a default parameter",
            result: None,
        }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let kind: String = call.req(1)?;
        let path: String = call.req(2)?;
        let patch = to_document(&call.req::<Value>(3)?)?;
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
    s.input_output_type(Type::Nothing, Type::Any)
        .required("expression", SyntaxShape::Any, "An expression as a list, or as DUST text.")
        .named("input", SyntaxShape::Record(vec![].into()), "Values for the expression's ?variables.", Some('i'))
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
        "A string argument is parsed as DUST text; any other value is the expression itself."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example { example: "stardust eval '[* ?n 2]' --input {n: 21}", description: "DUST text", result: None },
            Example { example: "stardust eval ['*' '?n' 2] --input {n: 21}", description: "A nu list", result: None },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let expr: Value = call.req(0)?;
        let span = expr.span();
        let expression = match &expr {
            Value::String { val, .. } => stardust::parse(val, Format::DUST).map_err(|e| sd_err(e, span))?,
            other => to_document(other)?,
        };
        let input = call.get_flag::<Value>("input")?.map(|v| to_document(&v)).transpose()?;
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

// ---------------------------------------------------------------- formats

simple!(FromDust, "from dust", |s: Signature| s.input_output_type(Type::String, Type::Any));
impl SimplePluginCommand for FromDust {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "from dust"
    }
    fn description(&self) -> &str {
        "Parse DUST text into structured data. Lets `open file.dust` work."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let span = input.span();
        let text = input.as_str()?;
        let doc = stardust::parse(text, Format::DUST).map_err(|e| sd_err(e, span))?;
        from_document(&doc, call.head)
    }
}

simple!(ToDust, "to dust", |s: Signature| {
    s.input_output_type(Type::Any, Type::String)
        .named("width", SyntaxShape::Int, "Wrap lines at this width (0: the default).", Some('w'))
});
impl SimplePluginCommand for ToDust {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "to dust"
    }
    fn description(&self) -> &str {
        "Format structured data as DUST text."
    }
    fn signature(&self) -> Signature {
        Self::sig()
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let width = call.get_flag::<i64>("width")?.unwrap_or(0);
        let bytes = to_document(input)?
            .serialize(Format::DUST, width)
            .map_err(|e| sd_err(e, call.head))?;
        let text = String::from_utf8(bytes)
            .map_err(|e| LabeledError::new("invalid UTF-8 from serializer").with_label(e.to_string(), call.head))?;
        Ok(Value::string(text, call.head))
    }
}
