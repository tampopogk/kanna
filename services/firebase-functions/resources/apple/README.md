# Apple trust anchors

Downloaded from Apple's PKI on 2026-09-15:

- https://www.apple.com/appleca/AppleIncRootCertificate.cer
- https://www.apple.com/certificateauthority/AppleRootCA-G2.cer
- https://www.apple.com/certificateauthority/AppleRootCA-G3.cer

The build copies this directory into `dist/resources/apple`. The verifier
loads only these bundled roots, with online certificate checks enabled and
only Production/Sandbox environments. Test signers live under `test/fixtures`
and are excluded from the Functions upload. No Xcode/local-signature mode is
available to deployed handlers.
