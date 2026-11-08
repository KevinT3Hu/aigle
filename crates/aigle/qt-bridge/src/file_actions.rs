//! File operations preserve sidecars and refuse destination catalog conflicts.
use super::*;

impl AigleEditor {
    pub(super) fn file_action(&mut self, r: &Value) -> Result<Value> {
        let from = PathBuf::from(r["path"].as_str().context("Source required")?);
        let action = r["action"].as_str().context("Action required")?;
        if matches!(action, "rename_copy" | "remove_copy") {
            let id = r["copy"].as_i64().filter(|id| *id > 0).context("Copy required")?;
            let key = PhotoKey { path: from.clone(), copy: Some(id) };
            self.target_meta(&key)?;
            let active = self.photo.as_ref().is_some_and(|p| p.copy == Some(id)
                && matches!(&p.source, SourceRef::File(path) if path == &from));
            if action == "rename_copy" {
                let name = r["destination"].as_str().context("Name required")?.trim();
                ensure!(!name.is_empty(), "Copy name cannot be empty");
                self.catalog.rename_copy(id, name)?;
            } else {
                // Remove first, then release the active document without saving it
                // back into the catalog. Missing originals do not prevent deletion.
                self.catalog.remove_copy(id)?;
                if active {
                    self.agent = None;
                    self.photo = None;
                }
            }
            return Ok(json!({"path":from,"copy":id,"action":action,"ok":true,
                "closed":active && action == "remove_copy"}));
        }
        ensure!(
            matches!(action, "move" | "relink" | "trash"),
            "Unknown file action"
        );
        let to = if action == "trash" {
            from.clone()
        } else {
            PathBuf::from(r["destination"].as_str().context("Destination required")?)
        };
        ensure!(
            from.is_absolute() && to.is_absolute(),
            "Absolute paths required"
        );
        if action != "trash" {
            ensure!(
                aigle_engine_api::is_photo_path(&to),
                "Destination must be a supported photo path"
            );
        }
        ensure!(
            self.export.is_none()
                && self
                    .import
                    .as_ref()
                    .is_none_or(|t| t.status()["running"] != true)
                && self
                    .duplicates
                    .as_ref()
                    .is_none_or(|t| t.status()["running"] != true),
            "Wait for export, import, and duplicate scanning to finish"
        );
        self.save()?;
        let active = self
            .photo
            .as_ref()
            .filter(|p| matches!(&p.source, SourceRef::File(path) if path == &from))
            .map(|p| p.copy);
        if active.is_some() {
            self.agent = None;
            self.photo = None;
        }
        let result = (|| -> Result<()> {
            match action {
                "move" if from != to => {
                    aigle_library::move_photo(&from, &to)?;
                    // Unlike move_photo on the catalog, relink refuses existing destination edits.
                    if let Err(error) = self.catalog.relink_photo(&from, &to) {
                        aigle_library::move_photo(&to, &from)
                            .with_context(|| format!("Catalog update failed ({error}); file rollback also failed. Photo may be at {}", to.display()))?;
                        return Err(error.into());
                    }
                }
                "relink" => {
                    let created = aigle_library::relink_sidecars(&from, &to)?;
                    if let Err(error) = self.catalog.relink_photo(&from, &to) {
                        for path in created {
                            std::fs::remove_file(path)?;
                        }
                        return Err(error.into());
                    }
                }
                "trash" => {
                    aigle_library::trash_photo(&from)?;
                    // Keep catalog identity/history: restoring files from the desktop
                    // trash at their original paths restores their catalog associations.
                }
                _ => {}
            }
            Ok(())
        })();
        self.store.forget(&from);
        self.store.forget(&to);
        let mut response =
            json!({"path":from,"destination":to,"action":action,"ok":result.is_ok()});
        if let Err(error) = result {
            response["error"] = json!(format!("{error:#}"));
        }
        if let Some(copy) = active {
            let reopen = if from.is_file() { &from } else { &to };
            if reopen.is_file() && action != "trash" && (from.is_file() || response["ok"] == true) {
                match self.open(SourceRef::File(reopen.clone()), copy) {
                    Ok(mut opened) => {
                        opened["path"] = json!(reopen);
                        response["opened"] = opened;
                    }
                    Err(error) => response["reopen_error"] = json!(error.to_string()),
                }
            } else if from.is_file() {
                match self.open(SourceRef::File(from.clone()), copy) {
                    Ok(mut opened) => {
                        opened["path"] = json!(from);
                        response["opened"] = opened;
                    }
                    Err(error) => response["reopen_error"] = json!(error.to_string()),
                }
            }
        }
        response["closed"] = json!(active.is_some() && self.photo.is_none());
        Ok(response)
    }
}
