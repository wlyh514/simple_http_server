use rustls::{server::ClientHello, ServerConfig};
use rustls_pemfile;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::{fs, io::BufReader, sync::Arc};

/// Convert certificate from PEM to DER.
fn load_certificates(filename: &str) -> Vec<CertificateDer<'static>> {
    let certificate_file: fs::File =
        fs::File::open(filename).expect("Cannot open .pem certificate file.");
    let mut reader: BufReader<fs::File> = BufReader::new(certificate_file);

    rustls_pemfile::certs(&mut reader)
        .map(|result| result.unwrap())
        .collect()
}

/// Convert decrypted private key from PEM (PKCS #1, PKCS #8, SEC #1) to DER.
fn load_private_key(filename: &str) -> PrivateKeyDer<'static> {
    let private_key_file: fs::File =
        fs::File::open(filename).expect("Cannot open .pem private key file.");
    let mut reader: BufReader<fs::File> = BufReader::new(private_key_file);

    loop {
        match rustls_pemfile::read_one(&mut reader).expect("Cannot read .pem private key file.") {
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => return key.into(),
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => return key.into(),
            Some(rustls_pemfile::Item::Sec1Key(key)) => return key.into(),
            None => break,
            _ => {}
        }
    }

    panic!(
        "No keys found in {:?}. Encrypted keys are not supported.",
        filename
    );
}

/// Configure TLS server by setting up certificates and private key.
fn config_tls() -> Arc<ServerConfig> {
    let certificates: Vec<_> = load_certificates("certificates/cert.pem");
    let private_key = load_private_key("certificates/decrypted_key.pem");

    let config: ServerConfig = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .expect("Bad certificate(s) or private key.");

    Arc::new(config)
}

/// Choose TLS server config based on ClientHello message.
pub fn choose_tls_config(_client_hello: ClientHello) -> Arc<ServerConfig> {
    config_tls()
}
