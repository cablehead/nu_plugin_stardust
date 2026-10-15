mod commands;
mod convert;
mod plugin;

fn main() {
    nu_plugin::serve_plugin(&plugin::StardustPlugin::default(), nu_plugin::MsgPackSerializer);
}
