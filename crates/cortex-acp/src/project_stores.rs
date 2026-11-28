//! Local owner administration. These operations have no ACP or MCP entry point.
use cortex_core::{
    CoreError, CoreResult, StoreChannel,
    store::{StoreRoot, administration, migration},
};
use std::ffi::OsString;
use workspace_service::native_routes::RouteEnvironment;

pub(crate) fn run(args: &[OsString]) -> CoreResult<()> {
    let channel = if workspace_protocol::channel::current().is_dev() {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    };
    let environment = cortex_acp::startup::channel_environment(RouteEnvironment::for_user()?);
    let root = StoreRoot::new(&environment.state, channel)?;
    match args {
        [operation, project, id, confirm]
            if operation == "return-to-project" && confirm == "--confirm" =>
        {
            migration::return_to_project(
                &root.project_key(std::path::Path::new(project))?,
                id.to_str()
                    .ok_or(CoreError::Refused("project_store_not_found"))?,
                true,
            )
        }
        [operation] if operation == "list" => {
            serde_json::to_writer_pretty(std::io::stdout().lock(), &administration::list(&root)?)?;
            println!();
            Ok(())
        }
        [operation, id, confirm]
            if (operation == "delete" || operation == "cleanup-original")
                && confirm == "--confirm" =>
        {
            let action = if operation == "delete" {
                administration::Removal::Database
            } else {
                administration::Removal::Originals
            };
            administration::remove(
                &root,
                id.to_str()
                    .ok_or(CoreError::Refused("project_store_not_found"))?,
                action,
                true,
            )
        }
        _ => Err(CoreError::Refused("invalid_project_store_command")),
    }
}
