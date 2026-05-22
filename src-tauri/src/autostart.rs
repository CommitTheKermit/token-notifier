use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutostartStatus {
    Enabled,
    RequiresApproval,
    NotRegistered,
    NotFound,
    Unsupported,
}

#[cfg(target_os = "macos")]
pub fn set_login_item_enabled(enabled: bool) -> anyhow::Result<AutostartStatus> {
    use smappservice_rs::{AppService, ServiceManagementError, ServiceStatus, ServiceType};

    let service = AppService::new(ServiceType::MainApp);
    if enabled {
        match service.register() {
            Ok(()) | Err(ServiceManagementError::AlreadyRegistered) => {}
            Err(error) => return Err(anyhow::anyhow!(error.to_string())),
        }
    } else {
        match service.unregister() {
            Ok(()) | Err(ServiceManagementError::JobNotFound) => {}
            Err(error) => return Err(anyhow::anyhow!(error.to_string())),
        }
    }

    Ok(match service.status() {
        ServiceStatus::Enabled => AutostartStatus::Enabled,
        ServiceStatus::RequiresApproval => AutostartStatus::RequiresApproval,
        ServiceStatus::NotRegistered => AutostartStatus::NotRegistered,
        ServiceStatus::NotFound => AutostartStatus::NotFound,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn set_login_item_enabled(_enabled: bool) -> anyhow::Result<AutostartStatus> {
    Ok(AutostartStatus::Unsupported)
}

#[cfg(target_os = "macos")]
pub fn login_item_status() -> AutostartStatus {
    use smappservice_rs::{AppService, ServiceStatus, ServiceType};

    let service = AppService::new(ServiceType::MainApp);
    match service.status() {
        ServiceStatus::Enabled => AutostartStatus::Enabled,
        ServiceStatus::RequiresApproval => AutostartStatus::RequiresApproval,
        ServiceStatus::NotRegistered => AutostartStatus::NotRegistered,
        ServiceStatus::NotFound => AutostartStatus::NotFound,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn login_item_status() -> AutostartStatus {
    AutostartStatus::Unsupported
}

#[cfg(target_os = "macos")]
pub fn open_login_items_settings() {
    smappservice_rs::AppService::open_system_settings_login_items();
}

#[cfg(not(target_os = "macos"))]
pub fn open_login_items_settings() {}
