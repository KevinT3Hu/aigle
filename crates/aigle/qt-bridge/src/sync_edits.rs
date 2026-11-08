use super::*;

fn selected_ops(r: &Value) -> Result<Vec<ParamOp>> {
    let params: EditParams = serde_json::from_value(r["params"].clone())?;
    let source = serde_json::to_value(params)?;
    let groups: Vec<String> = serde_json::from_value(r["groups"].clone())?;
    ensure!(
        !groups.is_empty() && groups.len() <= 17,
        "Choose adjustment groups"
    );
    let allowed = [
        "profile", "wb", "light", "hdr", "presence", "curve", "mixer", "bw", "grading", "detail",
        "lens", "effects", "raw", "geometry", "masks", "spots", "red_eye",
    ];
    let mut ops = vec![ParamOp::set(
        "process_version",
        source["process_version"].clone(),
    )];
    let mut seen = std::collections::HashSet::new();
    for group in groups {
        ensure!(
            allowed.contains(&group.as_str()),
            "Unknown adjustment group: {group}"
        );
        if seen.insert(group.clone()) {
            ops.push(ParamOp::set(&group, source[&group].clone()));
        }
    }
    Ok(ops)
}
impl AigleEditor {
    pub(super) fn selective_edits(&mut self, r: &Value) -> Result<Value> {
        let ops = selected_ops(r)?;
        if r["cmd"] == "paste_selected" {
            let p = self.photo()?;
            p.apply(
                EditCommand::user(p.doc.rev(), "Paste selected adjustments", ops),
                Interaction::Final,
            )?;
            p.dirty = p.doc.rev() != p.saved_rev;
            return Ok(Value::Null);
        }
        let targets = library_targets::targets(r)?;
        ensure!(
            !targets.is_empty() && targets.len() <= 10000,
            "Select 1 to 10000 targets"
        );
        self.save()?;
        let mut seen = std::collections::HashSet::new();
        let mut updated = vec![];
        let mut failures = vec![];
        for key in targets {
            let path = &key.path;
            if !seen.insert(key.clone()) {
                continue;
            }
            let result = (|| -> Result<()> {
                ensure!(path.is_file(), "Photo does not exist");
                // Match the full identity; a copy and its master are separate targets.
                if self.photo.as_ref().is_some_and(|p| {
                    p.copy == key.copy && matches!(&p.source, SourceRef::File(open) if open == path)
                }) {
                    let p = self.photo()?;
                    p.apply(
                        EditCommand::user(p.doc.rev(), "Sync selected adjustments", ops.clone()),
                        Interaction::Final,
                    )?;
                    p.dirty = true;
                    self.save()?;
                    return Ok(());
                }
                let mut meta = self.target_meta(&key)?;
                let original = meta.clone();
                let initial = meta.edits.clone().unwrap_or_default();
                let mut doc = if let Some(history) = self.catalog.history(&key)? {
                    PhotoDoc::restore(initial, history)?
                } else {
                    PhotoDoc::new(initial)?
                };
                doc.apply(
                    EditCommand::user(doc.rev(), "Sync selected adjustments", ops.clone()),
                    Interaction::Final,
                )?;
                meta.edits = Some((**doc.head()).clone());
                self.write_target_meta(&key, &meta)?;
                if let Err(error) = self.catalog.set_history(&key, &doc.saved_history()) {
                    self.write_target_meta(&key, &original).with_context(|| {
                        format!("History save failed ({error}); sidecar rollback failed")
                    })?;
                    return Err(error.into());
                }
                Ok(())
            })();
            match result {
                Ok(()) => updated.push(if r.get("targets").is_some() { json!({"path":path,"copy":key.copy}) } else { json!(path) }),
                Err(error) => failures.push(json!({"path":path,"copy":key.copy,"error":format!("{error:#}")})),
            }
        }
        Ok(json!({"updated":updated,"failures":failures}))
    }
}
