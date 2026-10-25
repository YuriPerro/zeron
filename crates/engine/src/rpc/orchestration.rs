use super::*;

impl EngineRpc {
    pub(super) async fn resolve_role_model(
        &self,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let p: crate::orchestration::rpc::ResolveRoleModelParams = parse_params(params)?;
        let role = crate::orchestration::rpc::load_role(self.repos.data_dir(), &p.role)?;
        let remote = p
            .target_device_id
            .as_deref()
            .filter(|target| *target != self.doc_host.device_id());
        let catalog = self.model_catalog(remote, p.harness).await?;
        role.resolve(&p, &catalog)
    }

    async fn model_catalog(
        &self,
        target: Option<&str>,
        harness: HarnessId,
    ) -> Result<Vec<zeron_proto::Model>, RpcError> {
        let mut params = serde_json::json!({ "harness": harness });
        if let Some(target) = target {
            params["targetDeviceId"] = serde_json::json!(target);
        }
        let origin = target.unwrap_or("this device");
        match Box::pin(RpcService::handle(self, methods::LIST_MODELS, params)).await? {
            RpcReply::Value(models) => serde_json::from_value(models)
                .map_err(|e| RpcError::Failed(format!("invalid model catalog from {origin}: {e}"))),
            RpcReply::Stream(_) => Err(RpcError::Failed(format!(
                "unexpected stream from {origin} for ListModels"
            ))),
        }
    }

    pub(super) fn create_orchestrator(
        &self,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let p: crate::orchestration::rpc::CreateOrchestratorParams = parse_params(params)?;
        let spaces: Vec<String> = self
            .workspace
            .read_spaces()
            .map_err(|e| RpcError::Failed(e.to_string()))?
            .into_iter()
            .map(|space| space.id)
            .collect();
        let data_dir = self.repos.data_dir();
        let config = crate::orchestration::rpc::create_orchestrator_entry(data_dir, p, &spaces)?;
        let chat_id = uuid::Uuid::new_v4().to_string();
        let created = self.mutate(MutateParams::CreateChat {
            chat_id: chat_id.clone(),
            space_id: None,
            device_id: Some(self.doc_host.device_id().to_string()),
            config: Some(crate::orchestration::rpc::chat_config(&config)),
            branch: None,
            cwd: None,
            parent_chat_id: None,
            cloud: false,
        });
        let titled = created.and_then(|()| {
            let renamed = self.workspace.rename_chat(&chat_id, &config.name);
            let outcome = match renamed {
                Ok(true) => Ok(()),
                Ok(false) => Err(RpcError::Failed(format!(
                    "chat {chat_id} disappeared before it was titled"
                ))),
                Err(e) => Err(RpcError::Failed(e.to_string())),
            };
            if outcome.is_err()
                && let Err(cleanup) = self.workspace.delete_chat(&chat_id)
            {
                tracing::warn!(%chat_id, %cleanup, "could not delete orchestrator chat after titling failed");
            }
            outcome
        });
        if let Err(err) = titled {
            if let Err(cleanup) =
                crate::orchestration::rpc::discard_orchestrator(data_dir, &config.id)
            {
                tracing::warn!(
                    orchestrator = %config.id,
                    %cleanup,
                    "could not remove orchestrator after chat creation failed"
                );
            }
            return Err(err);
        }
        RpcReply::value(&serde_json::json!({ "orchestratorId": config.id, "chatId": chat_id }))
    }
}
