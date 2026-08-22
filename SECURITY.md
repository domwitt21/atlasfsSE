# Security policy

## Supported versions

AtlasFS is pre-1.0 software. Security fixes are provided for the latest commit
on `main`; older commits and development builds are not supported.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's private
vulnerability reporting feature from the repository's **Security** tab. Include
the affected commit, platform, reproduction steps, impact, and any proposed
mitigation. If private reporting is unavailable, contact the repository owner
privately through their GitHub profile.

Please allow a reasonable period for triage and remediation before public
disclosure. You should receive an acknowledgement within seven days.

## Security boundaries

AtlasFS is experimental storage software, not a security boundary against a
malicious host administrator. It does not encrypt repositories or backups. The
embedded REST server binds to loopback by default, requires a bearer token, and
serves plain HTTP; remote deployments must use a trusted TLS reverse proxy and
a firewall. See `docs/THREAT_MODEL.md` for the complete model and residual risks.
