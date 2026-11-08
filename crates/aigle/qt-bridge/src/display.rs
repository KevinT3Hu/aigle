//! Monitor profile discovery runs separately from the command/render worker.
use super::*;

pub(super) struct Detection {
    pub result: std::sync::mpsc::Receiver<Value>,
    pub state: Value,
}
impl Detection {
    pub fn start(engine: Engine, screen: String) -> Self {
        let (tx, result) = std::sync::mpsc::channel();
        let state = json!({"screen":screen,"running":true,"profile":"Srgb"});
        std::thread::spawn(move || {
            let found = (|| -> Result<Value> {
                let path = if let Some(path) = std::env::var_os("AIGLE_DISPLAY_ICC").filter(|p| !p.is_empty()) {
                    PathBuf::from(path)
                } else { futures::executor::block_on(super::platform::monitor_profile(&screen))? };
                let imported = engine.import(ImportKind::DisplayProfile, std::fs::read(&path)?).wait()?;
                Ok(json!({"screen":screen,"running":false,"name":imported.name,"path":path,
                    "profile":DisplayProfileId::Icc(imported.data_ref.as_str().to_owned())}))
            })();
            let _ = tx.send(found.unwrap_or_else(|e| json!({"screen":screen,"running":false,"profile":"Srgb","error":format!("{e:#}")})));
        });
        Self { result, state }
    }
    pub fn poll(&mut self) {
        if self.state["running"] != true { return; }
        match self.result.try_recv() {
            Ok(value) => self.state = value,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.state["running"] = json!(false);
                self.state["error"] = json!("Display profile detection stopped");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
}
