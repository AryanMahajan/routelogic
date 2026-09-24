# Security

RouteLogic opens arbitrary repositories and sends arbitrary HTTP requests. Both deserve a
stated trust model rather than an assumed one.

## What RouteLogic reads

Only the directory you explicitly select, and only files that survive the `.gitignore`-aware
walk and the adapter's candidate filter.

Filesystem access is scoped by Tauri v2's capability system. The application has no ambient
access to your home directory; a folder becomes readable when you pick it in a dialog, and
that grant is what persists between sessions.

## What RouteLogic executes

**By default: nothing from your project.** Static discovery parses source into syntax trees.
It never imports a module, never evaluates an expression, never starts a server.

This is the main reason the default path is static rather than runtime. Opening an unfamiliar
repository — reviewing a pull request, evaluating a dependency, auditing something you do not
trust — is a normal thing to do, and it must not be equivalent to running that repository's
code.

**On explicit request: runtime enrich.** [Runtime enrich](discovery/runtime-enrich.md) imports
your application to ask it directly for its API. This executes project code, including any
import-time side effects. Therefore:

- It never runs automatically.
- Every run goes through a dialog that displays the exact command and interpreter it will
  use, and waits for confirmation.
- The target is remembered per project in `.routelogic/workspace.yaml` and revocable from
  the same dialog. The interpreter is never stored.
- The helper script is written to `.routelogic/local/` (gitignored) before it runs, so what
  is shown is what executes and can be read first.
- The helper introspects only. It starts no server, binds no port, and writes nothing —
  bytecode caching is disabled for the run.

Treat enabling runtime enrich as equivalent to running the project's test suite: fine for
your own code, a decision worth making consciously for someone else's.

## What RouteLogic sends

Only requests you trigger — or an agent you connected does. There is no telemetry, no
analytics, no update ping, no account, and no cloud component. The application is fully
functional with no network access beyond the requests you make.

## Agents

An [agent connected over MCP](agents.md) runs RouteLogic as its own process, started by the
agent's client — nothing listens on a port. What it can do is bounded in `rl-core`, not in
the agent's good behaviour:

- **Where it may send.** Loopback hosts, with any method. Any other host only if
  `agent.allow` in `.routelogic/workspace.yaml` lists it, optionally for some methods. The
  check is on the resolved host, not the environment's name, and repeats before every
  redirect. The file is read on every request, so revoking takes effect immediately.
- **What it sees.** Every secret value is replaced with `{{secret:NAME}}` before anything
  is returned: request echoes, response bodies and headers, captured values, errors.
- **What it writes.** Flows in `.routelogic/flows/`, and nothing else. It cannot edit
  environments, secrets, the allow list or the project's source through RouteLogic.
- **What is recorded.** Every request it sends, and every one it was refused, lands in
  history marked `agent`, redacted like any other.

The agent is only as trustworthy as its client and model, and it sees your API's real
responses — personal data in a local database included. Loopback is allowed by default
because that is the server you are developing; if yours talks to production data, run it
against a copy.

## Certificate verification

Certificate verification is **on** by default and can be disabled per request for local
development against self-signed certificates.

The toggle is per request, never global and never sticky, so disabling it for one call against
`localhost` cannot silently weaken a later call to a production host. Requests with
verification disabled are marked as such in history.

## Secret handling

Covered in full in [secrets](workspace/secrets.md). In summary:

- Secret values never enter committable files.
- Values live in the OS keychain where available, otherwise in a gitignored local file.
- Resolution happens in the HTTP engine immediately before sending, minimising the window in
  which a plaintext value exists.
- Secrets are redacted in history, exports, logs, and error messages.
- `.routelogic/.gitignore` is written at workspace creation, not after the fact.

## Redirects

Redirects are not followed by default. Beyond matching what an API client should do — show
you what the server actually returned — this avoids silently forwarding an `Authorization`
header to a host you did not intend to contact. When following is enabled, the full chain is
displayed.

## Threat model

**In scope**

- A malicious repository must not achieve code execution merely by being opened
- Credentials must not reach version control through normal use
- A parser must not crash or hang the application on hostile input

**Out of scope**

- RouteLogic does not sandbox runtime enrich. It runs project code with your privileges, which
  is why it is gated behind explicit consent
- RouteLogic does not defend against a malicious OpenAPI document beyond parser robustness
- Protecting secrets from other processes running as your user is the operating system's job

## Reporting a vulnerability

Not yet established — this project is pre-alpha and has no release. This section will name a
contact before the first public build.
