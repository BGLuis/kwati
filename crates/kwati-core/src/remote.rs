use std::sync::Arc;

use crate::{
    BackendError, ConnectionProfile, FtpBackend, KnownHosts, Protocol, ReconnectingBackend,
    RetryPolicy, Secrets, SftpBackend, SmbBackend,
};

/// Opens the backend for `profile`'s protocol, wrapped so that dropped connections are retried
/// and [`ReconnectingBackend::state`] can be shown to the user.
///
/// The first connection's error comes back unchanged, so the caller can tell
/// [`BackendError::host_key_unknown`], [`BackendError::authentication_failed`] and the other
/// refusals apart. Reconnections use the same profile, secrets and known hosts, so a key trusted
/// in the meantime is honoured and a changed one is still refused.
pub fn connect_profile(
    profile: &ConnectionProfile,
    secrets: Arc<dyn Secrets + Send + Sync>,
    known_hosts: KnownHosts,
) -> Result<ReconnectingBackend, BackendError> {
    let profile = profile.clone();
    ReconnectingBackend::new(
        move || {
            Ok(match profile.protocol {
                Protocol::Sftp => Box::new(SftpBackend::connect(
                    &profile,
                    secrets.as_ref(),
                    &known_hosts,
                )?),
                Protocol::Ftp | Protocol::Ftps => {
                    Box::new(FtpBackend::connect(&profile, secrets.as_ref())?)
                }
                Protocol::Smb => Box::new(SmbBackend::connect(&profile, secrets.as_ref())?),
            })
        },
        RetryPolicy::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthMethod, SecretsError};

    struct NoSecrets;

    impl Secrets for NoSecrets {
        fn get(&self, _: &str) -> Result<Option<String>, SecretsError> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> Result<(), SecretsError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), SecretsError> {
            Ok(())
        }
    }

    fn profile(protocol: Protocol, start_path: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: "p".into(),
            name: "n".into(),
            protocol,
            host: "127.0.0.1".into(),
            port: 9,
            user: "u".into(),
            start_path: start_path.into(),
            auth: AuthMethod::Password,
            private_key_path: None,
            allow_insecure: false,
        }
    }

    /// Each protocol reaches its own backend: the refusals below are checks only that backend
    /// makes, before any network traffic.
    #[test]
    fn each_protocol_is_opened_by_its_own_backend() {
        let refusal = |protocol, start_path, auth| {
            connect_profile(
                &ConnectionProfile {
                    auth,
                    ..profile(protocol, start_path)
                },
                Arc::new(NoSecrets),
                KnownHosts::new("/nonexistent/known_hosts"),
            )
            .err()
            .unwrap()
            .to_string()
        };
        let key = AuthMethod::PrivateKey;
        let password = AuthMethod::Password;
        assert!(refusal(Protocol::Smb, "/", password).contains("share name"));
        assert!(refusal(Protocol::Sftp, "/", key).contains("names no key file"));
        // FTP ignores the auth method and asks for the password.
        assert!(refusal(Protocol::Ftp, "/", key).contains("no password"));
        assert!(refusal(Protocol::Ftps, "/", key).contains("no password"));
    }
}
