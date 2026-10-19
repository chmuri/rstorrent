# Security Policy

## Supported versions

| Version | Supported |
| ------- | --------- |
| 0.10.x  | Yes       |
| < 0.10  | No        |

## Reporting a vulnerability

Please do **not** open a public issue for security problems.

- Use GitHub's private vulnerability reporting:
  https://github.com/chmuri/rstorrent/security/advisories/new
- Or contact the maintainer directly at `chmuri@gmail.com`.

Include:

- affected version / commit,
- a description of the issue and its impact,
- reproduction steps or a proof of concept,
- any suggested mitigation.

You can expect an acknowledgement within 72 hours. Once the issue is confirmed,
a fix and a coordinated disclosure will be prepared; credit will be given unless
you prefer to stay anonymous.

## Scope

In scope:

- the `rstorrent` daemon, the SCGI/XML-RPC/JSON-RPC and gRPC servers,
- the Torrent engine, protocol and storage crates,
- the Docker images and packaging scripts.

Out of scope:

- issues that require an already-trusted local user with access to the RPC
  socket/port (the daemon exposes the same trust model as rTorrent),
- third-party frontends (Flood, ruTorrent).
