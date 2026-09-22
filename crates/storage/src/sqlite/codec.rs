//! The column-to-domain mapping shared by get and list.
use super::*;

pub(super) fn profile(row: &rusqlite::Row<'_>) -> Result<BrowserProfile, StorageError> {
    let id_raw: String = row.get(0)?;
    let name: String = row.get(1)?;
    let core_id_raw: String = row.get(2)?;
    let user_data_dir_str: String = row.get(3)?;
    let fingerprint_json: String = row.get(4)?;
    let proxy_id_raw: Option<String> = row.get(5)?;
    let width: u32 = row.get(6)?;
    let height: u32 = row.get(7)?;
    let start_target_json: String = row.get(8)?;

    let profile_id = ProfileId(
        Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
    );
    let core_id = CoreId(
        Uuid::parse_str(&core_id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
    );
    let proxy_id = match proxy_id_raw {
        Some(p) => Some(ProxyId(
            Uuid::parse_str(&p).map_err(|e| StorageError::Serialization(e.to_string()))?,
        )),
        None => None,
    };
    let fingerprint: FingerprintProfile = serde_json::from_str(&fingerprint_json)
        .map_err(|e| StorageError::Serialization(e.to_string()))?;
    let start_target: StartTarget = serde_json::from_str(&start_target_json)
        .map_err(|e| StorageError::Serialization(e.to_string()))?;

    Ok(BrowserProfile {
        id: profile_id,
        name,
        core_id,
        user_data_dir: PathBuf::from(user_data_dir_str),
        fingerprint,
        proxy_id,
        window: WindowProfile::new(width, height),
        start_target,
    })
}

pub(super) fn proxy(row: &rusqlite::Row<'_>) -> Result<ProxyProfile, StorageError> {
    let id_raw: String = row.get(0)?;
    let name: String = row.get(1)?;
    let outbound_json: String = row.get(2)?;

    let proxy_id =
        ProxyId(Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?);
    let outbound: ProxyOutbound = serde_json::from_str(&outbound_json)
        .map_err(|e| StorageError::Serialization(e.to_string()))?;

    Ok(ProxyProfile {
        id: proxy_id,
        name,
        outbound,
    })
}

pub(super) fn core(row: &rusqlite::Row<'_>) -> Result<BrowserCore, StorageError> {
    let id_raw: String = row.get(0)?;
    let name: String = row.get(1)?;
    let executable_str: String = row.get(2)?;
    let version: String = row.get(3)?;
    let major: u32 = row.get(4)?;

    let core_id =
        CoreId(Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?);

    Ok(BrowserCore {
        id: core_id,
        name,
        executable: PathBuf::from(executable_str),
        version,
        major,
    })
}
