use super::*;
use crate::{
    infrastructure::harness_installation::{self as installation, Event},
    model::harness_installation::{InstallMethod, MaintenanceRequest},
};

impl Presenter {
    fn configured_harnesses(&self) -> Vec<(HarnessKind, String)> {
        HarnessKind::ALL
            .into_iter()
            .map(|harness| {
                let executable = if harness == self.model.selected_harness {
                    self.model.executable.clone()
                } else {
                    self.storage
                        .setting(executable_setting_key(harness))
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| harness.default_executable().into())
                };
                (harness, executable)
            })
            .collect()
    }

    pub(crate) fn scan_harness_installations(&mut self) {
        if self.model.harness_manager.busy {
            return;
        }
        match installation::spawn(self.configured_harnesses(), None) {
            Ok(worker) => {
                self.installation_worker = Some(worker);
                self.model.harness_manager.busy = true;
                self.model.harness_manager.message = Some("正在扫描 Harness 安装…".into());
            }
            Err(error) => self.model.harness_manager.message = Some(error.to_string().into()),
        }
    }

    pub(crate) fn harness_maintenance_request(
        &self,
        harness: HarnessKind,
        method: Option<InstallMethod>,
    ) -> Option<MaintenanceRequest> {
        if self.model.harness_manager.busy || self.model.active_run_count() > 0 {
            return None;
        }
        let installation = self.model.harness_manager.installations.get(&harness)?;
        if !self
            .configured_harnesses()
            .iter()
            .any(|(kind, configured)| *kind == harness && *configured == installation.configured)
        {
            return None;
        }
        let command = match method {
            Some(method) => installation
                .install_options
                .iter()
                .find(|option| option.method == method)?
                .command
                .clone(),
            None => installation.update.clone()?,
        };
        Some(MaintenanceRequest {
            harness,
            configured: installation.configured.clone(),
            executable: installation.executable.clone(),
            method,
            command,
        })
    }

    pub(crate) fn maintain_harness(&mut self, request: MaintenanceRequest) -> bool {
        let Some(current) = self.harness_maintenance_request(request.harness, request.method)
        else {
            return false;
        };
        if current.command != request.command
            || current.executable != request.executable
            || current.configured != request.configured
        {
            self.model.harness_manager.message = Some("安装来源已变化，请重新扫描后再试。".into());
            return false;
        }
        match installation::spawn(self.configured_harnesses(), Some(request.clone())) {
            Ok(worker) => {
                self.installation_worker = Some(worker);
                self.model.harness_manager.busy = true;
                self.model.harness_manager.operating = Some(request.harness);
                self.model.harness_manager.message = Some(LocalizedText::new(
                    "正在安装或更新 {harness}…",
                    &[("harness", request.harness.to_string())],
                ));
                true
            }
            Err(error) => {
                self.model.harness_manager.message = Some(error.to_string().into());
                false
            }
        }
    }

    pub(crate) fn cancel_harness_maintenance(&mut self) {
        if let Some(worker) = &self.installation_worker {
            worker.cancel();
        }
    }

    pub(crate) fn drain_installation_events(&mut self) -> bool {
        let Some(worker) = &self.installation_worker else {
            return false;
        };
        let mut events = Vec::new();
        let disconnected = loop {
            match worker.events.try_recv() {
                Ok(event) => events.push(event),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        let changed = !events.is_empty() || disconnected;
        for event in events {
            match event {
                Event::Scanned(harness, mut installation) => {
                    if self
                        .configured_harnesses()
                        .iter()
                        .any(|(kind, configured)| {
                            *kind == harness && *configured == installation.configured
                        })
                    {
                        if installation.discovered_from_manager
                            && installation.version.is_some()
                            && self.model.active_run_count() == 0
                            && let Some(path) = &installation.executable
                        {
                            let path = path.display().to_string();
                            match self
                                .storage
                                .set_setting(executable_setting_key(harness), &path)
                            {
                                Ok(()) => {
                                    installation.configured = path.clone();
                                    installation.discovered_from_manager = false;
                                    if self.model.selected_harness == harness {
                                        self.model.executable = path;
                                    }
                                    if self.model.harness_manager.operating != Some(harness) {
                                        self.refresh_installed_harness(harness);
                                    }
                                }
                                Err(error) => {
                                    installation.diagnostic = Some(LocalizedText::new(
                                        "无法保存 Harness 路径：{error}",
                                        &[("error", error.to_string())],
                                    ))
                                }
                            }
                        }
                        self.model
                            .harness_manager
                            .installations
                            .insert(harness, installation);
                    }
                }
                Event::Finished { request, result } => {
                    self.installation_worker = None;
                    self.model.harness_manager.busy = false;
                    self.model.harness_manager.operating = None;
                    self.model.harness_manager.message = Some(match result {
                        Err(error) => error,
                        Ok(()) if request.is_some() => "操作完成，已重新检测版本。".into(),
                        Ok(()) => "Harness 扫描完成。".into(),
                    });
                    // Even a failed installer may have changed files. Refresh readiness
                    // without switching the user's selected harness or executable override.
                    if let Some(request) = request {
                        self.refresh_installed_harness(request.harness);
                    }
                }
            }
        }
        if disconnected && self.installation_worker.is_some() {
            self.installation_worker = None;
            self.model.harness_manager.busy = false;
            self.model.harness_manager.operating = None;
            self.model.harness_manager.message = Some("Harness 管理任务已中断，请重试。".into());
        }
        changed
    }

    fn refresh_installed_harness(&mut self, harness: HarnessKind) {
        self.model.harnesses.remove(&harness);
        if let Some(runner) = &self.runner
            && let Some((_, executable)) = self
                .configured_harnesses()
                .into_iter()
                .find(|(kind, _)| *kind == harness)
        {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness,
                executable,
            }));
        }
        if harness == self.model.selected_harness {
            self.model.model_catalog = ModelCatalogState::Idle;
            self.refresh_model_catalog();
        }
        if harness == self.model.title_generation.harness {
            self.model.title_model_catalog = ModelCatalogState::Idle;
        }
    }
}
