# Streaming transport security

## RustDJ 0.20 and later

The default **TLS (certificate verified)** mode protects both the SHOUTcast
source connection and metadata updates. It verifies the certificate chain,
validity and hostname/IP using rustls and bundled Mozilla trust roots before
application credentials are sent. TLS 1.2 and TLS 1.3 are supported. There is no
automatic plaintext fallback or option to accept an invalid certificate.

Loading a settings file without a transport-security field defaults to TLS,
while preserving the saved host, ports and encrypted password. The provider must
offer TLS-enabled source and metadata endpoints, or a correctly configured TLS
proxy. Existing unencrypted ports are not automatically converted to secure ports.

**Unencrypted legacy TCP / HTTP** is a deliberate compatibility option. It sends
the source password and metadata password without transport protection and is
clearly marked in the UI. Use it only on a trusted local or separately encrypted
connection. This mode does not claim TLS protection.

The legacy SHOUTcast metadata API includes the password in the request query.
HTTPS encrypts that query on the network, but the receiving server or TLS proxy
can still see it and may store it in access logs. Redact sensitive query fields
in server/proxy logging. RustDJ does not include passwords or requests in its
connection-error messages.

Windows DPAPI protects the saved password for the current Windows account.
Local storage encryption and transport encryption protect different boundaries.
Streaming and recording never start automatically.

## Versions through 0.19

These versions sent source credentials over plain TCP and metadata credentials
in a plain HTTP query. DPAPI protected only the saved local password. Anyone
with access to the corresponding network traffic could read these credentials.

Upgrade to 0.20 or later, select TLS and use the provider's secure endpoints.
Rotate credentials previously used over an untrusted network. Until the server
supports a secure connection, stop broadcasting over that network or use a
separately secured connection.

## Verification

`cargo test --lib tls20_` runs two targeted tests using ephemeral certificates
and local servers. They verify encrypted source/audio and metadata transmission,
certificate/hostname/expiry failures, rejection of plaintext peers without
downgrade, bounded cancellation and secure settings defaults. The tests inspect
raw TCP bytes for cleartext credentials and requests. Test trust roots are
injected only in test builds; certificate verification remains enabled.
