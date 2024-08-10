use std::env::VarError;

use anyhow::bail;
use libdav::auth::{Auth, Password};

pub fn auth_from_env() -> anyhow::Result<Auth> {
    match std::env::var("DAVCLI_USERNAME") {
        Ok(username) => {
            let password = match std::env::var("DAVCLI_PASSWORD").map(Password::from) {
                Ok(password) => Some(password),
                Err(VarError::NotPresent) => None,
                Err(VarError::NotUnicode(_)) => {
                    bail!("password must be valid unicode");
                }
            };
            Ok(Auth::Basic { username, password })
        }
        Err(VarError::NotPresent) => Ok(Auth::None),
        Err(VarError::NotUnicode(_)) => {
            bail!("username must be valid unicode");
        }
    }
}
