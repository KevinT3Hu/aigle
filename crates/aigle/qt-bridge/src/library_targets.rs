use super::*;

/// Accept legacy master paths or explicit identities. Never discard a supplied copy ID.
pub(super) fn targets(r: &Value) -> Result<Vec<PhotoKey>> {
    if let Some(values) = r.get("targets") {
        values.as_array().context("Targets must be an array")?.iter().map(|v| {
            Ok(PhotoKey {
                path: PathBuf::from(v["path"].as_str().context("Target path required")?),
                copy: match v.get("copy") {
                    None | Some(Value::Null) => None,
                    Some(id) => Some(id.as_i64().filter(|id| *id > 0).context("Invalid virtual copy ID")?),
                },
            })
        }).collect()
    } else {
        let paths: Vec<PathBuf> = serde_json::from_value(r["paths"].clone())?;
        Ok(paths.into_iter().map(PhotoKey::master).collect())
    }
}

impl AigleEditor {
    pub(super) fn create_library_copies(&mut self, r: &Value) -> Result<Value> {
        let targets = targets(r)?;
        ensure!(!targets.is_empty() && targets.len() <= 10000, "Select 1 to 10000 photos");
        let base = r["name"].as_str().unwrap_or("Copy").trim();
        ensure!(!base.is_empty(), "Copy name cannot be empty");
        self.save()?;
        let mut seen = std::collections::HashSet::new();
        // Validate every identity before creating anything; never substitute a master.
        let sources = targets.into_iter().filter(|key| seen.insert(key.clone()))
            .map(|key| self.target_meta(&key).map(|meta| (key, meta)))
            .collect::<Result<Vec<_>>>()?;
        let mut created = vec![];
        let mut errors = vec![];
        for (key, mut meta) in sources {
            meta.snapshots.clear();
            let result = (|| -> Result<Value> {
                let existing = self.catalog.copies_of(std::slice::from_ref(&key.path))?;
                let mut name = base.to_owned();
                let mut suffix = 2;
                while existing.iter().any(|copy| copy.name == name) {
                    name = format!("{base} {suffix}");
                    suffix += 1;
                }
                let id = self.catalog.create_copy(&key.path, &name, &meta)?;
                Ok(json!({"id":id,"path":key.path,"name":name}))
            })();
            match result {
                Ok(copy) => created.push(copy),
                Err(error) => errors.push(json!({"path":key.path,"copy":key.copy,"error":error.to_string()})),
            }
        }
        Ok(json!({"copies":created,"errors":errors}))
    }

    pub(super) fn target_meta(&self, key: &PhotoKey) -> Result<PhotoMeta> {
        if let Some(id) = key.copy {
            // Validate ownership, including deleted IDs, before any read or mutation.
            self.catalog.copies_of(std::slice::from_ref(&key.path))?
                .into_iter().find(|copy| copy.id == id)
                .map(|copy| copy.meta).context("Virtual copy does not belong to this photo")
        } else {
            Ok(self.store.read(&key.path)?.meta)
        }
    }
    pub(super) fn write_target_meta(&self, key: &PhotoKey, meta: &PhotoMeta) -> Result<()> {
        if let Some(id) = key.copy {
            self.target_meta(key)?;
            self.catalog.set_copy_meta(id, meta)?;
        } else {
            self.store.write(&key.path, meta)?;
        }
        Ok(())
    }
}

impl AigleEditor {
    pub(super) fn export_inputs(&self, r: &Value) -> Result<Value> {
        if r["current"] == true {
            let photo = self.photo.as_ref().context("Open a photo first")?;
            let name = match &photo.source {
                SourceRef::File(path) => path.file_name().map(|v| v.to_string_lossy().into_owned()).unwrap_or("export".into()),
                _ => "Generated.png".into(),
            };
            if let SourceRef::File(path) = &photo.source {
                return self.export_inputs(&json!({"targets":[{"path":path,"copy":photo.copy}]}));
            }
            return Ok(json!([{"source_ref":photo.source,"params":photo.doc.head(),"name":name,"raw":false}]));
        }
        let targets = targets(r)?;
        ensure!(!targets.is_empty() && targets.len() <= 10000, "Select 1 to 10000 photos");
        let mut seen = std::collections::HashSet::new();
        let mut inputs = vec![];
        for key in targets {
            if !seen.insert(key.clone()) { continue; }
            ensure!(key.path.is_file(), "Photo does not exist: {}", key.path.display());
            let meta = self.target_meta(&key)?;
            let active = self.photo.as_ref().filter(|p| p.copy == key.copy &&
                matches!(&p.source, SourceRef::File(path) if path == &key.path));
            let params = active.map(|p| (**p.doc.head()).clone()).unwrap_or_else(|| meta.edits.unwrap_or_default());
            let mut name = key.path.file_name().map(|v| v.to_string_lossy().into_owned()).unwrap_or("export".into());
            if let Some(id) = key.copy {
                let copy = self.catalog.copies_of(std::slice::from_ref(&key.path))?.into_iter()
                    .find(|c| c.id == id).context("Virtual copy no longer exists")?;
                let stem = key.path.file_stem().unwrap_or_default().to_string_lossy();
                let extension = key.path.extension().map(|v| format!(".{}", v.to_string_lossy())).unwrap_or_default();
                name = format!("{stem} ({}){extension}", copy.name);
            }
            inputs.push(json!({"source_ref":SourceRef::File(key.path.clone()),"params":params,
                "name":name,"raw":aigle_engine_api::is_raw_path(&key.path),"path":key.path,"copy":key.copy}));
        }
        Ok(json!(inputs))
    }
}
