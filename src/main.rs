mod commands;
mod convert;
mod objects;
mod plugin;
mod pool;

fn main() {
    nu_plugin::serve_plugin(&plugin::StardustPlugin::default(), nu_plugin::MsgPackSerializer);
}
