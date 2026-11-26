use super::*;

impl Session {
    pub async fn update_meta(&self, f: impl FnOnce(&mut SessionMeta)) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let mut meta = self.meta();
        f(&mut meta);
        anyhow::ensure!(meta.id == self.inner.id, "cannot change session id");
        meta.updated_at_ms = now_millis();
        let store = self.inner.store.clone();
        let inner = self.inner.clone();
        storage::blocking(move || {
            let _guard = guard;
            store.immediate(|tx| storage::update_meta(tx, &meta))?;
            *write_lock(&inner.meta) = meta;
            Ok(())
        })
        .await
    }
    pub async fn set_title(&self, title: impl Into<String>) -> Result<()> {
        let title = title.into();
        self.update_meta(|meta| meta.title = Some(title)).await
    }
    pub async fn set_model(&self, model: impl Into<String>) -> Result<()> {
        let model = model.into();
        self.update_meta(|meta| meta.model = Some(model)).await
    }
    pub async fn set_provider(&self, provider: impl Into<String>) -> Result<()> {
        let provider = provider.into();
        self.update_meta(|meta| meta.provider = Some(provider))
            .await
    }
    pub async fn set_approval_mode(&self, approval_mode: impl Into<String>) -> Result<()> {
        let approval_mode = approval_mode.into();
        self.update_meta(|meta| meta.approval_mode = Some(approval_mode))
            .await
    }
    pub async fn set_remote_session(
        &self,
        harness: impl Into<String>,
        remote_session_id: impl Into<String>,
    ) -> Result<()> {
        let harness = harness.into();
        let remote_session_id = remote_session_id.into();
        self.update_meta(|meta| {
            meta.harness = Some(harness);
            meta.remote_session_id = Some(remote_session_id);
        })
        .await
    }
    pub async fn clear_remote_session(&self) -> Result<()> {
        self.update_meta(|meta| {
            meta.harness = None;
            meta.remote_session_id = None;
        })
        .await
    }
}
