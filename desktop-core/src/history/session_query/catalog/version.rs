use super::*;

impl CatalogIndex {
    pub fn version(
        &self,
        request: &SessionQueryParams,
        generation: u64,
    ) -> Result<CatalogVersion, RpcError> {
        self.retention_current()?;
        if identity(&crate::history::delivery::upload_queue_path(&self.path))?
            != self.queue_identity
        {
            return Err(failed());
        }
        let data_version: u64 = self
            .delivery
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(|_| failed())?;
        self.version_at(
            request,
            CatalogVersion {
                revision: 0,
                data_version,
                generation,
            },
        )
    }
    pub(super) fn version_at(
        &self,
        request: &SessionQueryParams,
        snapshot: CatalogVersion,
    ) -> Result<CatalogVersion, RpcError> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        let mut request = request.clone();
        request.known_revision = None;
        (
            self.index.epoch,
            self.index.revision,
            snapshot.data_version,
            snapshot.generation,
            serde_json::to_string(&request).map_err(|_| failed())?,
        )
            .hash(&mut hash);
        Ok(CatalogVersion {
            revision: (hash.finish() & i64::MAX as u64).max(1),
            ..snapshot
        })
    }
}
