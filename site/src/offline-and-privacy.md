# Offline and privacy

Self-hosted first. A hosted provider is an adapter you switch on, never a fallback.

## Deployment profiles

Every pipeline is declared under a deployment profile. A profile row outside this table is refused
(`ProfileOutsideTable`).

| Profile | Admitted compute | Default egress |
|---|---|---|
| `offline_strict` | in-process or local workers only | denied |
| `self_hosted_lan` | local machine and approved internal services | internal allowlist |
| `self_hosted_cluster` | approved owned infrastructure | authenticated internal paths |
| `hybrid_opt_in` | approved self-hosted and explicitly selected remote services | per-provider allowlist |

Only `hybrid_opt_in` admits an adapter at locality `external_provider`.

## `offline_strict`, enforced twice <span class="st st-rel">0.6.0</span>

1. **At planning**: an adapter whose locality the profile does not admit is refused with
   `LocalityDenied`, before anything runs. A local worker on loopback, such as `llama-server` on
   127.0.0.1, is admitted.
2. **At run time**, the egress watch: on Linux, a run whose profile denies egress fails once any
   socket of the process connects to an address that is neither loopback nor unspecified, or holds
   an unconnected datagram socket not bound to loopback. The failure names the address.

The egress watch is a tripwire and a test instrument, **not a sandbox**:

- it polls, so a socket opened and closed between two polls is missed;
- it prevents nothing: bytes of a connection it sees may already have left;
- it reads every socket of the process, so it cannot tell which node opened one;
- it does not read packet sockets, SCTP, or another process's sockets;
- it exists on Linux only.

If you need isolation, run the process in a network namespace or behind a firewall as well.

## Licence policy for local adapters

- Backend code is Apache-2.0 or MIT, or more permissive.
- Pinned weights are CC BY 4.0, or more permissive.
- Each checkpoint's licence, source URL and hash go into the adapter's capability record when it is
  adopted. Any other licence needs an explicit decision for that checkpoint.
- Checkpoints are pinned by sha256; an adapter refuses a file that differs, naming it.
- Tests download no weights and start no model server: a test that needs one reads its path or URL
  from an environment variable and is ignored without it.

The licences of each shipped checkpoint are on [Adapters](adapters.md). The local TTS adapter waits
on this policy: sherpa-onnx's TTS build links GPL-3.0 code.

## Credentials

Adapters never look a key up themselves. The OpenAI adapter takes an injected credential source
(environment, or the OS keychain behind a feature); Nova Sonic uses the AWS SDK's default chain.
AWS service errors are shown with ARNs and account ids redacted.
