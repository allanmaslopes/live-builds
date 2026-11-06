use std::time::Duration;

use ureq::tls::{Certificate, RootCerts, TlsConfig};

pub(super) fn agent() -> Result<ureq::Agent, String> {
    let root = Certificate::from_pem(include_bytes!("../../assets/riotgames.pem"))
        .map_err(|error| error.to_string())?;
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::new_with_certs(&[root]))
        .build();

    Ok(ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_millis(500)))
        // The game API must stay on loopback, even when a system proxy is configured.
        .proxy(None)
        .max_redirects(0)
        .build()
        .new_agent())
}
