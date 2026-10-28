//! Plugin state: open databases behind handles, and the process license.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nu_plugin::{EngineInterface, EvaluatedCall, Plugin, PluginCommand};
use nu_protocol::{CustomValue, LabeledError, ShellError, Span, Spanned, Value};
use serde::{Deserialize, Serialize};
use stardust::Database;

use crate::commands;
use crate::convert::sd_err;

#[derive(Default)]
pub struct StardustPlugin {
    /// Open databases by handle ID, each with the path and plugins it was
    /// opened with. Handles to one path share one `Database`; it closes when
    /// the last handle and the last stream holding its `Arc` are gone.
    dbs: Mutex<HashMap<u64, Open>>,
    next_id: AtomicU64,
    licensed: Mutex<bool>,
}

impl Plugin for StardustPlugin {
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    fn commands(&self) -> Vec<Box<dyn PluginCommand<Plugin = Self>>> {
        commands::all()
    }

    fn custom_value_to_base_value(
        &self,
        _engine: &EngineInterface,
        custom_value: Spanned<Box<dyn CustomValue>>,
    ) -> Result<Value, LabeledError> {
        Ok(custom_value.item.to_base_value(custom_value.span)?)
    }

    fn custom_value_dropped(
        &self,
        engine: &EngineInterface,
        custom_value: Box<dyn CustomValue>,
    ) -> Result<(), LabeledError> {
        if let Some(handle) = custom_value.as_any().downcast_ref::<DbHandle>() {
            self.close(engine, handle.id)?;
        }
        Ok(())
    }
}

/// A handle's database, and what it was opened as.
struct Open {
    path: String,
    plugins: String,
    db: Arc<Database>,
}

impl StardustPlugin {
    /// Activates the license once per process. The library reads no
    /// environment or file itself, so the token comes from
    /// `$env.STARDUST_LICENSE` or `~/.config/stardust/license`.
    pub fn ensure_licensed(&self, engine: &EngineInterface, span: Span) -> Result<(), LabeledError> {
        let mut licensed = self.licensed.lock().expect("license lock");
        if *licensed {
            return Ok(());
        }
        let token = match engine.get_env_var("STARDUST_LICENSE")? {
            Some(value) => value.coerce_into_string()?,
            None => {
                let home = match engine.get_env_var("HOME")? {
                    Some(home) => home.coerce_into_string()?,
                    None => std::env::var("HOME").unwrap_or_default(),
                };
                let path = format!("{home}/.config/stardust/license");
                std::fs::read_to_string(&path).map_err(|e| {
                    LabeledError::new("no Stardust license")
                        .with_label(format!("could not read {path}: {e}"), span)
                        .with_help("set $env.STARDUST_LICENSE, or save the token to ~/.config/stardust/license")
                })?
            }
        };
        stardust::activate(token.trim()).map_err(|e| sd_err(e, span))?;
        *licensed = true;
        Ok(())
    }

    /// The database this process already has open at `path`, if any, and the
    /// plugins it was opened with. Stardust locks a file to one open, so a
    /// second `stardust open` of it (a hot reload's, say, while the old
    /// handler still serves) shares this one instead of failing `locked`.
    pub fn opened(&self, path: &str) -> Option<(Arc<Database>, String)> {
        let dbs = self.dbs.lock().expect("db lock");
        dbs.values().find(|o| o.path == path).map(|o| (o.db.clone(), o.plugins.clone()))
    }

    pub fn register(&self, engine: &EngineInterface, path: String, plugins: String, db: Arc<Database>) -> Result<u64, ShellError> {
        // The plugin's pid in the high bits: a handle outlives this process
        // when nu restarts the plugin (`plugin stop`), and its drop, which
        // reaches the new process, must not close a database there that took
        // the same count.
        let id = (u64::from(std::process::id()) << 32) | self.next_id.fetch_add(1, Ordering::Relaxed);
        self.dbs.lock().expect("db lock").insert(id, Open { path, plugins, db });
        // An idle plugin is stopped by nu's GC, which would close every database.
        engine.set_gc_disabled(true)?;
        Ok(id)
    }

    pub fn close(&self, engine: &EngineInterface, id: u64) -> Result<bool, ShellError> {
        let mut dbs = self.dbs.lock().expect("db lock");
        // The database closes with its last Arc, which calls into Stardust:
        // drop it on the pool.
        let removed = dbs.remove(&id);
        let found = removed.is_some();
        crate::pool::drop(removed);
        if dbs.is_empty() {
            engine.set_gc_disabled(false)?;
        }
        Ok(found)
    }

    /// The database behind positional argument `index`.
    pub fn db(&self, call: &EvaluatedCall, index: usize) -> Result<Arc<Database>, LabeledError> {
        let value: Value = call.req(index)?;
        let span = value.span();
        let handle = DbHandle::from_value(&value)?;
        self.dbs.lock().expect("db lock").get(&handle.id).map(|o| o.db.clone()).ok_or_else(|| {
            LabeledError::new("database is closed")
                .with_label(format!("{} is no longer open", handle.path), span)
                .with_help("closed with `stardust close`, or the plugin restarted: open it again")
        })
    }
}

/// What nu holds for an open database: an ID into the plugin's table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbHandle {
    pub id: u64,
    pub path: String,
}

impl DbHandle {
    pub fn from_value(value: &Value) -> Result<&DbHandle, LabeledError> {
        let span = value.span();
        match value {
            Value::Custom { val, .. } => val.as_any().downcast_ref::<DbHandle>(),
            _ => None,
        }
        .ok_or_else(|| {
            LabeledError::new("expected a database")
                .with_label(format!("this is a {}", value.get_type()), span)
                .with_help("get one with `stardust open <path>`")
        })
    }
}

#[typetag::serde]
impl CustomValue for DbHandle {
    fn clone_value(&self, span: Span) -> Value {
        Value::custom(Box::new(self.clone()), span)
    }

    fn type_name(&self) -> String {
        "stardust-db".into()
    }

    fn to_base_value(&self, span: Span) -> Result<Value, ShellError> {
        Ok(Value::string(format!("stardust:{}", self.path), span))
    }

    fn notify_plugin_on_drop(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
