# How to Use Backend Functions

A backend function is the app developer's code that the deployment runs on
request (`plan-backend-functions.md`). This guide covers what is built.

## Secrets

An app secret is a key or token a function sends to an outside API, such as
an OpenAI key. The function never holds the value. It holds a handle, and
the egress kernel puts the value into the request as it leaves.

### Declare it in the program

```ordo
secret openaiKey for api.openai.com
secret mistralKey for api.mistral.ai, eu.mistral.ai
```

- The name is how a function asks for it.
- The hosts are the only places the value is ever sent.
- A host is an exact name. A URL, a port or a wildcard is refused
  (`invalid_secret_host`).
- A host whose first label starts with a digit is written in quotes:
  `secret k for "1password.example"`.
- One line per name. A second line for the same name is `duplicate_secret`.
- A binding is part of the program, so changing a host is a new version.

### Set the value

The deployment owner sets it through the door. The value is read from
standard input, never from the command line:

```sh
ordo secret set openaiKey --host <h> --space <s> < openai-key.txt
```

- The value is sealed under the deployment's keys in `$secrets`, in the
  control database.
- Each set is a new version.
- Setting needs a passkey touch by default, like every platform change.
- The deployment's security record gets one `secret` entry per set: the
  name, the version and who set it. Never the value.
- A value may be set before the program declares the secret. It is sent
  nowhere until a declaration binds it.

### List and remove

```sh
ordo secret ls --host <h> --space <s>
ordo secret rm openaiKey --host <h> --space <s>
```

- `ls` shows each name, its bound hosts, its version, and who set it when.
- No command and no op reads a value back.
- `rm` removes the value. A function that uses it is then refused with
  `secret_not_set`.

The ops behind these are `std.deployment.setSecret`,
`std.deployment.secrets` and `std.deployment.removeSecret`. They answer only
the deployment owner, only in the deployment's own space.

### Per environment

Each deployment is one environment, with its own control database and its
own keys. Set each secret in each deployment. Staging's value never reaches
production.

### What a function's fetch is refused for

Every refusal comes back with one reason word:

| Word | Meaning | What to do |
|---|---|---|
| `host_not_allowed` | the host is not on the function's `egress` list | add it to the function |
| `host_denied` | the host resolves inside the deployment | use a public host |
| `secret_not_listed` | the function does not list the secret | add it to the function's `secrets` |
| `secret_unknown` | the program declares no such secret | add a `secret` line |
| `secret_not_set` | no value is set in this deployment | `ordo secret set <name>` |
| `secret_host_mismatch` | the secret is not bound to this host | bind the host, or call the bound one |
| `body_too_large` | a request or response body is over its limit | send or ask for less |
| `timed_out` | the request took longer than its time | try again |

`secret_host_mismatch` is checked before any DNS lookup. So code steered to
another allowed host still cannot carry the key there.

### Limits

| Limit | Default |
|---|---|
| Request body | 10 MB |
| Response body | 25 MB |
| One request's time | 60 s |
| A secret's value | 64 KiB |

A redirect is returned to the function as an answer. It is never followed.
If an answer repeats a secret's value, the value is replaced with `[secret]`
before the function sees it.
