//! The objects plugin: payloads stored once per SHA-256, named by a tree of
//! files and directories. One command per operation of the SDK's typed view,
//! `stardust objects <operation>`. Open the database with
//! `--plugins {objects: {}}` to use them.
//!
//! A digest is the `{#sha256 hex}` record Stardust returns; the commands
//! also take the hex text alone.

use std::io::{self, Read, Write};
use std::sync::Arc;

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand, SimplePluginCommand};
use nu_protocol::{
    ByteStream, ByteStreamType, Category, Example, LabeledError, PipelineData, Signature, SyntaxShape, Type, Value,
};
use stardust::{Database, Document, PluginStream};

use crate::commands::{db_arg, document, format, format_flag};
use crate::convert::{from_document, sd_err};
use crate::plugin::StardustPlugin;
use crate::pool;

pub fn all() -> Vec<Box<dyn PluginCommand<Plugin = StardustPlugin>>> {
    vec![
        Box::new(Objects),
        Box::new(Upload),
        Box::new(Open),
        Box::new(Get),
        Box::new(Lookup),
        Box::new(Children),
        Box::new(Path),
        Box::new(Files),
        Box::new(Manifest),
        Box::new(Create),
        Box::new(Update),
        Box::new(Move),
        Box::new(Remove),
    ]
}

fn sig(name: &str) -> Signature {
    db_arg(Signature::build(name)).category(Category::Database)
}

/// A payload digest from `{#sha256 hex}` or the hex text.
fn digest(value: &Value) -> Result<[u8; 32], LabeledError> {
    let span = value.span();
    let hex = match value {
        Value::String { val, .. } => Some(val.as_str()),
        Value::Record { val, .. } => match val.get("#sha256") {
            Some(Value::String { val, .. }) => Some(val.as_str()),
            _ => None,
        },
        _ => None,
    };
    let bad = || {
        LabeledError::new("expected a digest")
            .with_label("not a SHA-256: {#sha256 <64 hex digits>}, or the hex alone", span)
    };
    let hex = hex.ok_or_else(bad)?;
    if hex.len() != 64 {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// Runs one call of the typed view and returns its document as nu data.
fn call_view(
    plugin: &StardustPlugin,
    engine: &EngineInterface,
    call: &EvaluatedCall,
    f: impl FnOnce(&Database) -> stardust::Result<Document>,
) -> Result<Value, LabeledError> {
    plugin.ensure_licensed(engine, call.head)?;
    let db = plugin.db(call, 0)?;
    let doc = f(&db).map_err(|e| sd_err(e, call.head))?;
    from_document(&doc, call.head)
}

// ---------------------------------------------------------------- objects

pub struct Objects;
impl SimplePluginCommand for Objects {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects"
    }
    fn description(&self) -> &str {
        "The objects plugin: payloads stored once per SHA-256, named by a tree of files and directories."
    }
    fn extra_description(&self) -> &str {
        "Open the database with `--plugins {objects: {}}`. `upload` publishes bytes and returns their digest; `create` names a digest as a file. Directory 0 is the root. Payloads are never deleted, and a file's history keeps every payload it named."
    }
    fn signature(&self) -> Signature {
        Signature::build("stardust objects").category(Category::Database)
    }
    fn run(&self, _: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        Ok(Value::string(engine.get_help()?, call.head))
    }
}

// ---------------------------------------------------------------- upload

pub struct Upload;
impl PluginCommand for Upload {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects upload"
    }
    fn description(&self) -> &str {
        "Publish the input's bytes as a payload: {digest size deduplicated}."
    }
    fn extra_description(&self) -> &str {
        "The input streams through: a file, an external command's output or any binary or string. Bytes already published are stored once, and deduplicated is true."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects upload").input_output_types(vec![
            (Type::Binary, Type::record()),
            (Type::String, Type::record()),
            (Type::Nothing, Type::record()),
        ])
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example { example: "open --raw logo.png | stardust objects upload $db", description: "Publish a file's bytes", result: None },
            Example { example: "'Hello, objects' | stardust objects upload $db | get digest", description: "Publish text and get its digest", result: None },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: PipelineData) -> Result<PipelineData, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let span = call.head;
        let mut stream = db.objects().upload().map_err(|e| sd_err(e, span))?;
        let written = match input {
            PipelineData::Empty => Ok(()),
            PipelineData::Value(Value::Nothing { .. }, _) => Ok(()),
            PipelineData::Value(Value::Binary { val, .. }, _) => stream.write_all(&val),
            PipelineData::Value(Value::String { val, .. }, _) => stream.write_all(val.as_bytes()),
            PipelineData::ByteStream(bytes, _) => match bytes.reader() {
                Some(mut reader) => copy(&mut reader, &mut stream),
                None => Ok(()),
            },
            other => {
                return Err(LabeledError::new("expected bytes")
                    .with_label(format!("got {}", other.get_type()), span)
                    .with_help("pipe in binary, a string or a byte stream, e.g. `open --raw <file>`"));
            }
        };
        // Dropping the stream abandons the upload.
        written.map_err(|e| LabeledError::new("upload failed").with_label(e.to_string(), span))?;
        let published = stream.finish().map_err(|e| sd_err(e, span))?;
        Ok(PipelineData::value(from_document(&published, span)?, None))
    }
}

/// Copies in chunks, so a long input never sits in memory whole.
fn copy(reader: &mut impl Read, stream: &mut PluginStream) -> io::Result<()> {
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            return Ok(());
        }
        stream.write_all(&buffer[..n])?;
    }
}

// ---------------------------------------------------------------- open

pub struct Open;
impl PluginCommand for Open {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects open"
    }
    fn description(&self) -> &str {
        "Stream a payload's bytes, or the part --offset and --length pick."
    }
    fn extra_description(&self) -> &str {
        "Each chunk read is checked against its digest. The output is text when the bytes are UTF-8; `| into binary` keeps it bytes."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects open")
            .input_output_type(Type::Nothing, Type::Any)
            .required("digest", SyntaxShape::Any, "The payload's digest: {#sha256 hex}, or the hex.")
            .named("offset", SyntaxShape::Int, "Start this many bytes in (default 0).", None)
            .named("length", SyntaxShape::Int, "Read at most this many bytes (default: to the end).", None)
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example { example: "stardust objects open $db $digest", description: "A payload's bytes", result: None },
            Example { example: "stardust objects open $db $digest --offset 7 --length 7", description: "Bytes 7 to 13", result: None },
        ]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: PipelineData) -> Result<PipelineData, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let span = call.head;
        let digest = digest(&call.req::<Value>(1)?)?;
        let mut stream = db.objects().open(digest).map_err(|e| sd_err(e, span))?;
        if let Some(offset) = call.get_flag::<i64>("offset")? {
            stream.seek(offset, 0).map_err(|e| sd_err(e, span))?;
        }
        let left = match call.get_flag::<i64>("length")? {
            Some(n) if n < 0 => return Err(LabeledError::new("invalid length").with_label("must be 0 or more", span)),
            Some(n) => Some(n as u64),
            None => None,
        };
        let reader = PayloadReader { _db: db, stream: Some(stream), left };
        Ok(PipelineData::byte_stream(
            ByteStream::read(reader, span, engine.signals().clone(), ByteStreamType::Unknown),
            None,
        ))
    }
}

/// Reads a payload as nu pulls it. Holds the database so it stays open until
/// the read ends, even if its handle is dropped first.
struct PayloadReader {
    _db: Arc<Database>,
    stream: Option<PluginStream>,
    left: Option<u64>,
}

impl Read for PayloadReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let max = match self.left {
            Some(left) => buf.len().min(left.try_into().unwrap_or(usize::MAX)),
            None => buf.len(),
        };
        let Some(stream) = self.stream.as_mut() else { return Ok(0) };
        if max == 0 {
            pool::drop(self.stream.take());
            return Ok(0);
        }
        let n = pool::run(|| stream.read(&mut buf[..max])).map_err(|e| io::Error::other(e.to_string()))?;
        if let Some(left) = &mut self.left {
            *left -= n as u64;
        }
        if n == 0 {
            pool::drop(self.stream.take());
        }
        Ok(n)
    }
}

impl Drop for PayloadReader {
    fn drop(&mut self) {
        pool::drop(self.stream.take());
    }
}

// ---------------------------------------------------------------- the tree

pub struct Get;
impl SimplePluginCommand for Get {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects get"
    }
    fn description(&self) -> &str {
        "A file's or directory's record: {entity parent name kind revision}, and a file's size and digest."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects get")
            .input_output_type(Type::Nothing, Type::record())
            .required("entity", SyntaxShape::Int, "The node's entity.")
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let entity: i64 = call.req(1)?;
        call_view(plugin, engine, call, |db| db.objects().get(entity))
    }
}

pub struct Lookup;
impl SimplePluginCommand for Lookup {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects lookup"
    }
    fn description(&self) -> &str {
        "The record of the child of a directory with a name."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects lookup")
            .input_output_type(Type::Nothing, Type::record())
            .required("parent", SyntaxShape::Int, "The directory's entity; 0 is the root.")
            .required("name", SyntaxShape::String, "The child's name.")
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example { example: "stardust objects lookup $db 0 notes", description: "The root's child named notes", result: None }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let parent: i64 = call.req(1)?;
        let name: String = call.req(2)?;
        call_view(plugin, engine, call, |db| db.objects().lookup(parent, &name))
    }
}

pub struct Children;
impl SimplePluginCommand for Children {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects children"
    }
    fn description(&self) -> &str {
        "The records of a directory's children, in name order."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects children")
            .input_output_type(Type::Nothing, Type::table())
            .optional("parent", SyntaxShape::Int, "The directory's entity (default: 0, the root).")
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let parent: i64 = call.opt(1)?.unwrap_or(0);
        call_view(plugin, engine, call, |db| db.objects().children(parent))
    }
}

pub struct Path;
impl SimplePluginCommand for Path {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects path"
    }
    fn description(&self) -> &str {
        "The names from the root to a node."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects path")
            .input_output_type(Type::Nothing, Type::list(Type::String))
            .required("entity", SyntaxShape::Int, "The node's entity.")
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example { example: "stardust objects path $db 7 | str join /", description: "A node's path as text", result: None }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let entity: i64 = call.req(1)?;
        call_view(plugin, engine, call, |db| db.objects().path(entity))
    }
}

pub struct Files;
impl SimplePluginCommand for Files {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects files"
    }
    fn description(&self) -> &str {
        "The entities of the files that name a payload, ascending."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects files")
            .input_output_type(Type::Nothing, Type::list(Type::Int))
            .required("digest", SyntaxShape::Any, "The payload's digest: {#sha256 hex}, or the hex.")
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let digest = digest(&call.req::<Value>(1)?)?;
        call_view(plugin, engine, call, |db| db.objects().files(digest))
    }
}

pub struct Manifest;
impl SimplePluginCommand for Manifest {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects manifest"
    }
    fn description(&self) -> &str {
        "A payload's {digest size chunks}: its content-defined chunks, {digest size} each, in byte order."
    }
    fn signature(&self) -> Signature {
        sig("stardust objects manifest")
            .input_output_type(Type::Nothing, Type::record())
            .required("digest", SyntaxShape::Any, "The payload's digest: {#sha256 hex}, or the hex.")
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let digest = digest(&call.req::<Value>(1)?)?;
        call_view(plugin, engine, call, |db| db.objects().manifest(digest))
    }
}

pub struct Create;
impl SimplePluginCommand for Create {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects create"
    }
    fn description(&self) -> &str {
        "Create files and directories in one commit; their records, in order."
    }
    fn extra_description(&self) -> &str {
        "Each node is {name}: a directory, or a file with a digest. parent is an existing directory (default 0, the root); under is the one-based position of a directory earlier in the same list. A refused list creates nothing."
    }
    fn signature(&self) -> Signature {
        format_flag(sig("stardust objects create"))
            .input_output_types(vec![(Type::list(Type::record()), Type::table()), (Type::String, Type::table())])
    }
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "let d = 'Hello' | stardust objects upload $db | get digest\n[{name: notes} {name: hello.txt under: 1 digest: $d}] | stardust objects create $db",
            description: "A directory, and a file inside it",
            result: None,
        }]
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, input: &Value) -> Result<Value, LabeledError> {
        let nodes = document(input, format(call)?)?;
        call_view(plugin, engine, call, |db| db.objects().create(&nodes))
    }
}

/// `--expected-revision`: a write's check that nothing changed the node since
/// it was read. 0, the default, skips it.
fn expected_revision(sig: Signature) -> Signature {
    sig.named(
        "expected-revision",
        SyntaxShape::Int,
        "Refuse with conflict unless the node is at this revision (default: 0, no check).",
        None,
    )
}

pub struct Update;
impl SimplePluginCommand for Update {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects update"
    }
    fn description(&self) -> &str {
        "Point a file at another published payload; its record."
    }
    fn extra_description(&self) -> &str {
        "The previous payload stays published, and the file's history keeps it."
    }
    fn signature(&self) -> Signature {
        expected_revision(sig("stardust objects update"))
            .input_output_type(Type::Nothing, Type::record())
            .required("entity", SyntaxShape::Int, "The file's entity.")
            .required("digest", SyntaxShape::Any, "The payload's digest: {#sha256 hex}, or the hex.")
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let entity: i64 = call.req(1)?;
        let digest = digest(&call.req::<Value>(2)?)?;
        let expected = call.get_flag::<i64>("expected-revision")?.unwrap_or(0);
        call_view(plugin, engine, call, |db| db.objects().update(entity, digest, expected))
    }
}

pub struct Move;
impl SimplePluginCommand for Move {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects move"
    }
    fn description(&self) -> &str {
        "Rename a node, or move it to another directory; its record."
    }
    fn signature(&self) -> Signature {
        expected_revision(sig("stardust objects move"))
            .input_output_type(Type::Nothing, Type::record())
            .required("entity", SyntaxShape::Int, "The node's entity.")
            .required("parent", SyntaxShape::Int, "The destination directory; 0 is the root.")
            .required("name", SyntaxShape::String, "The node's new name.")
            .switch("replace", "Replace a node of the same kind at the destination (an empty one, for a directory).", None)
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        let entity: i64 = call.req(1)?;
        let parent: i64 = call.req(2)?;
        let name: String = call.req(3)?;
        let expected = call.get_flag::<i64>("expected-revision")?.unwrap_or(0);
        let replace = call.has_flag("replace")?;
        call_view(plugin, engine, call, |db| db.objects().r#move(entity, parent, &name, expected, replace))
    }
}

pub struct Remove;
impl SimplePluginCommand for Remove {
    type Plugin = StardustPlugin;
    fn name(&self) -> &str {
        "stardust objects remove"
    }
    fn description(&self) -> &str {
        "Remove a file or an empty directory. A file's payload stays published."
    }
    fn signature(&self) -> Signature {
        expected_revision(sig("stardust objects remove"))
            .input_output_type(Type::Nothing, Type::Nothing)
            .required("entity", SyntaxShape::Int, "The node's entity.")
    }
    fn run(&self, plugin: &StardustPlugin, engine: &EngineInterface, call: &EvaluatedCall, _: &Value) -> Result<Value, LabeledError> {
        plugin.ensure_licensed(engine, call.head)?;
        let db = plugin.db(call, 0)?;
        let entity: i64 = call.req(1)?;
        let expected = call.get_flag::<i64>("expected-revision")?.unwrap_or(0);
        db.objects().remove(entity, expected).map_err(|e| sd_err(e, call.head))?;
        Ok(Value::nothing(call.head))
    }
}
