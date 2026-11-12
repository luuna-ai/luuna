# Security policy

Luuna is an early development project. No released version currently has a supported security maintenance window. Do not use the foundation as a production trust boundary or assume that agent tools are sandboxed.

## Report a vulnerability privately

Use this repository's GitHub **Security → Report a vulnerability** option when private vulnerability reporting is enabled. Otherwise contact the repository owner through an established private channel to arrange disclosure. Do not put credentials, exploitable details, private transcripts, or affected users' data in a public issue. A public issue may request a private reporting channel without disclosing the vulnerability.

Include the affected revision, reproduction steps using synthetic data, expected and actual behavior, and impact. Remove secrets and personal information from logs before sharing. Do not test against other users' systems or provider accounts.

## Credentials and repository hygiene

Store provider/GitHub credentials in private user configuration or secure environment bindings. Never commit real `.env` files, session histories, credential files, private memory, or transcripts. Ignore rules do not remove already tracked data or erase Git history.

If a credential is exposed, revoke or rotate it first, review use, and coordinate any necessary history remediation separately. Removing a file in a later commit does not remove earlier copies, forks, caches, or downloaded artifacts. Secret scanning reduces risk but cannot establish that no secret has ever existed.

## Runtime boundaries

The runtime must authenticate local API actions, bind to loopback by default, validate exact approvals, restrict artifact access, supervise owned children, and record interrupted execution. A desktop connection or model response is not authorization. Native process execution has host permissions; stronger isolation requires an explicitly implemented operating-system or container boundary.

Treat repository instructions, imported tool records, agent output, and memory as untrusted inputs. Preserve upstream license notices and review imported development artifacts before using them as model context. Required validation, independent review, publication reconciliation, and the developer's control of merging in their own repositories (Luuna's runtime has no automatic merge operation in v1) must not be bypassed to recover from a failure.

How changes to this repository itself are reviewed, merged and released is set by [ADR 0044](docs/decisions/0044-one-repository-and-agent-shipping.md) and does not change these runtime boundaries.
