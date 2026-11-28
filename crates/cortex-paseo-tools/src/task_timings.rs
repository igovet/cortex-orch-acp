use cortex_core::{observability, CoreResult, OpenMode, Store, StoreChannel};

pub fn run(argv: &[String]) -> (String, i32) {
    let (root, task, channel) = match argv {
        [root, task] => (root, task, StoreChannel::Release),
        [root, task, flag] if flag == "--dev" => (root, task, StoreChannel::Dev),
        _ => {
            return (
                "usage: cortex-paseo-tools task-timings PROJECT_ROOT EXECUTION_ID [--dev]\n".into(),
                2,
            );
        }
    };
    let result = (|| -> CoreResult<String> {
        let key = crate::store::project_key(root, channel)?;
        let store = Store::open_waiting(key, OpenMode::LiveReadOnly)?;
        let timings = observability::task_timings(&store, task)?;
        Ok(serde_json::to_string_pretty(&timings)? + "\n")
    })();
    match result {
        Ok(output) => (output, 0),
        Err(error) => (format!("{{\"error\":\"{}\"}}\n", error.code()), 1),
    }
}
