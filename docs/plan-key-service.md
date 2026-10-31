# An outside key service for the master key

**Checklist 18y, after the first deployment. Answers security finding SH-11.
Written 2026-09-29 at the owner's word:** *"ideally it is indeed in a key
management service and we need to implement that. Make sure it is not
already implemented, if it is not then plan it for post-deploy."*

## 1. Is it already built? No.

Checked against the code on 2026-09-29.

| What | Where | State |
|---|---|---|
| The seam a key service plugs into | `crates/ordo-crypto/src/custody.rs`, trait `KeyProvider` | Built |
| The built-in provider | `custody.rs`, `LocalKeyProvider` | Built |
| Master key from the environment | `crates/ordo-crypto/src/master.rs` | Built |
| Master key from a file, under a passphrase | `master.rs` | Built |
| Any provider that talks to a key service | nowhere | **Not built** |
| Any key service client in the dependencies | no `Cargo.toml` names one | **Not built** |

`KeyProvider` has two implementations: `LocalKeyProvider` and the test
provider. The trait was shaped for this. It exposes operations and never
keys, so a provider can keep a key somewhere the caller cannot reach.

## 2. What is true today

- The server reads the master key at boot, from the environment or a file.
- The master key stays in the server's memory while it runs.
- Each space's key is stored wrapped under the master, and unwrapped when
  the space is used.
- Somebody who takes over the running server has the master key, and so
  every space's key. That is SH-11.

## 3. Two designs, and what each buys

### A. The key service wraps the master key

The master key sits in a file, wrapped by the key service. At boot the
server asks the service to unwrap it once.

- **Buys:** the key is not on the disk and not in the environment.
- **Does not buy:** the key is still in memory. `master.rs` already says so
  in plain words. SH-11 stays open.
- **Cost:** small. One call at boot.
- **Risk:** the key service being down stops a server from starting.

### B. The key service wraps each space's key

The master key never leaves the key service. When a space is used, the
server asks the service to unwrap that space's key, and keeps it for a short
time.

- **Buys:** a taken-over server holds only the keys of spaces used in the
  last few minutes. Access can be cut off at the service. The service logs
  every unwrap, outside the server's reach.
- **Does not buy:** keys that never touch memory. Sealing a field needs the
  space's key in memory while it works.
- **Cost:** one call per space per time window, not per field. A call per
  field is what `master.rs` rules out as unaffordable.
- **Risk:** the key service being down stops any space whose key has
  expired from memory.

### Recommendation

**B, with the file and environment sources kept for small and self-hosted
deployments.** A alone does not answer SH-11, which is the reason this plan
exists. The easy deployment stays as it is.

## 4. What must not change

- Self-hosting without any cloud account stays first-class (ruled).
- Postgres stays the only stateful dependency a small deployment needs.
- Moving to a key service is a swap and a re-wrap, never a migration. The
  envelopes, the epochs and every digest stay as they are.
- Blind-index digests must not move. They come from the space's key, which
  B leaves unchanged.

## 5. Increments

| Step | What | Proves |
|---|---|---|
| KS1 | A `KeyService` trait: wrap and unwrap, nothing else. A fake for tests | The seam, with no cloud account |
| KS2 | A provider that unwraps each space's key through it, kept for a bounded time | Design B against the fake |
| KS3 | `ordo keys rewrap-master --to <service>`: move a deployment onto it | A swap and a re-wrap, no digest moves |
| KS4 | Vault and OpenBao, runnable in docker-compose or a small cluster | One real unwrap against a local Vault in a test |
| KS4b | GCP KMS, AWS KMS, Azure Key Vault, and Cloudflare if it has one | One real unwrap each, in tests that need a credential |
| KS5 | What happens when the service is down: boot, serving, readiness | A named refusal instead of a hang |
| KS6 | The custody guide and the deploying guide | An operator can set it up from the docs |

KS1 to KS3 need no cloud account and can be built and tested locally.

## 6. Answered by the owner, 2026-09-29

1. **Both designs**, leaning on the recommendation: B is the default, A is
   offered for a deployment that only wants the key off the disk.
2. **Which services:** GCP KMS, AWS KMS, Cloudflare if it offers a key
   service, and HashiCorp Vault. Also any other large one (Azure Key Vault is
   the obvious candidate) and a popular self-hosted one (OpenBao, the open
   fork of Vault, which the Vault provider can likely serve as well). The
   owner: *"realize that for most people this will probably be run in a small
   k8s cluster or docker-compose deployment."* So the self-hosted provider,
   Vault or OpenBao beside the server in the same compose file or cluster,
   is the first real one to build (KS4), and the cloud services follow.
   Whether Cloudflare has a suitable key service is to be checked when KS4
   starts, not assumed.
3. **Five minutes in memory, adjustable.**

**The easy deployment, agreed 2026-09-29** (`ez-deploy-keys`): *"Agree with
this setup - set bao for post deploy."*

| Where it runs | Key design |
|---|---|
| docker-compose on any machine (first), or a small Kubernetes cluster | A, the sealed key file, by default. Built. |
| The same, on a GCP or AWS machine | C, the cloud key service, unlocked by the machine's own identity, offered |
| Anyone who runs OpenBao or wants to unlock by hand | B, OpenBao in the same compose file, an option after the first deployment |

A key service beside the server has to unlock itself on restart, so on one
machine it needs a key on that machine. That is why OpenBao is an option and
not the default. This changes the order of KS4: the cloud services come
before OpenBao.

## 7. Related

- 18j, rotating the master key as an operation, is a separate item. A key
  service makes it simpler, since the service rotates its own key.
- 17b still lists "any KMS binding" as owed. This plan is that binding.
- Ruled 2026-09-21 (`which-kms`): v1 reads credentials from a mounted
  folder. This plan adds an option after v1 and does not change that.
