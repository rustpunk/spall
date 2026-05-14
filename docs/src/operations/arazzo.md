# Arazzo Workflows

[Arazzo 1.0.1](https://spec.openapis.org/arazzo/latest.html) is the
OpenAPI Initiative's standard for describing multi-step API workflows —
"log in, capture the token, use it to fetch the user, assert the user is
active." Spall ships a v1 runner that reads `.arazzo.yaml` files,
resolves their `sourceDescriptions` against the existing IR cache, and
executes each step through the same request pipeline used by every
other spall command.

> **Status:** v1, single-direction (stops on first failure). Failure
> actions, nested workflows, `replay`, regex/jsonpath criteria, and the
> `--spall-bind` CLI override are tracked in
> [issue #5](https://github.com/rustpunk/spall/issues/5).

## Subcommands

```text
spall arazzo run <file>
    [--input key=value]    repeatable; populates $inputs.<key>
    [--workflow id]        choose when the doc has >1 workflow
    [--dry-run]            print resolved requests, send nothing
    [--output json|yaml]   serialization for final workflow outputs (stdout)
    [--verbose]            emit a source-binding banner at workflow start

spall arazzo validate <file>
    # parses the doc and reports any v2-only constructs it found
```

Workflow stdout is **only** the final outputs object — pipe-clean for
`jq`, shell `read`, or downstream tools. Progress, warnings, and
dry-run details go to stderr.

## A minimal workflow

```yaml
# onboard.arazzo.yaml
arazzo: 1.0.1
info:
  title: Onboard a new customer
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: ./customer-openapi.json
    type: openapi
workflows:
  - workflowId: createAndFetch
    inputs:
      type: object
      properties:
        email: { type: string }
    steps:
      - stepId: createUser
        operationId: createUser
        requestBody:
          contentType: application/json
          payload:
            email: $inputs.email
        successCriteria:
          - condition: $response.statusCode == 201
        outputs:
          user_id: $response.body#/id
      - stepId: fetchUser
        operationId: getUser
        parameters:
          - name: id
            in: path
            value: $steps.createUser.outputs.user_id
        successCriteria:
          - condition: $response.statusCode == 200
        outputs:
          email: $response.body#/email
    outputs:
      user_id: $steps.createUser.outputs.user_id
      email: $steps.fetchUser.outputs.email
```

```bash
spall arazzo run ./onboard.arazzo.yaml --input email=alice@example.com
```

Output (stdout, JSON):

```json
{
  "outputs": { "email": "alice@example.com", "user_id": "user-42" },
  "steps": [ … ],
  "workflowId": "createAndFetch"
}
```

## The expression dialect

Step parameters, request-body fields, success criteria, and outputs all
accept Arazzo expressions. The runner evaluates any string that starts
with `$`; everything else is treated as a literal.

| Expression                                       | Resolves to                                |
|--------------------------------------------------|--------------------------------------------|
| `$inputs.email`                                  | The `--input email=...` value              |
| `$workflow.inputs.region`                        | Alias for `$inputs.region`                 |
| `$steps.<id>.outputs.<name>`                     | A named output from an earlier step        |
| `$steps.<id>.response.body#/path/to/field`       | RFC 6901 JSON Pointer into the step body   |
| `$steps.<id>.response.header.X-Request-Id`       | Response header (RFC 9110 case-insensitive) |
| `$steps.<id>.response.statusCode`                | Integer status code                        |
| `$response.body#/foo` (only in outputs/criteria) | Current step's response body               |
| `$response.header.<Name>`                        | Current step's response headers            |
| `$response.statusCode`                           | Current step's status code                 |

When an expression appears inside a JSON request body, the runner walks
the structure and replaces every string leaf that starts with `$`. A
non-string leaf (`true`, `42`, an array, an object) passes through
unchanged.

## Runtime conditions for `successCriteria`

Each `successCriteria[].condition` is one of:

- A **bare expression** that the runner asserts is truthy. The falsy
  values are `false`, `null`, `0`, `""`, `[]`, and `{}`.
- A **binary comparison** `<lhs> <op> <rhs>` where each operand is an
  expression or a literal (number, `true`/`false`/`null`, or a quoted
  string), and `<op>` is one of `==`, `!=`, `<`, `<=`, `>`, `>=`.

```yaml
successCriteria:
  - condition: $response.statusCode == 200
  - condition: $response.body#/status == "ready"
  - condition: $steps.create.outputs.count > 0
  - condition: $response.body#/items   # truthy if the array is non-empty
```

`<` / `<=` / `>` / `>=` require both operands to coerce to a number;
otherwise the step fails with `operand cannot be coerced to a number`.

## Binding sources to spall APIs

Each `sourceDescription` names an OpenAPI spec. The runner resolves the
URL (file path or `http(s)://`) through spall's existing fetch +
IR-cache pipeline, then binds the source to a configured spall API
entry so it can reuse the API's auth chain. Resolution order:

1. **Explicit override:** if the source description has
   `x-spall-api: <api-name>`, bind to that spall API.
2. **Name match:** if `<source.name>` matches a registered
   `spall api`, bind to it.
3. **Synthetic:** otherwise, synthesize a bare API entry — requests run
   unauthenticated. The runner emits a stderr warning with the exact
   fix-it command.

```yaml
sourceDescriptions:
  - name: petstore
    url: https://petstore3.swagger.io/api/v3/openapi.json
    type: openapi
    # default: binds to `spall api petstore` if it exists.
  - name: internal
    url: ./internal-api.json
    type: openapi
    x-spall-api: prod   # bind to the configured `spall api prod` instead
```

If a step receives a 401 or 403 from an unbound source, the error
message tells you exactly how to fix it:

```text
step 'getMe': step 'getMe' returned HTTP 401 — source 'petstore' is
unbound; try: spall api add petstore https://… && spall auth login petstore
```

## What runs unauthenticated

When the source is synthetic (no matching spall API entry), the runner
does not attach any Authorization header. Workflows that target a
public API or local mock are fine; workflows that target an
authenticated API need either:

```bash
spall api add petstore https://example.com/openapi.json
spall auth login petstore                  # set up creds
spall arazzo run ./workflow.arazzo.yaml    # auth chain runs automatically
```

…or an `x-spall-api: <existing-api>` extension on the source
description.

## Validation

`spall arazzo validate <file>` parses the document and surfaces:

- **Errors** — the doc declares no workflows, or a step has neither
  `operationId` nor `operationPath` nor `workflowId`. Exit code 10.
- **Warnings** — the doc uses `operationPath` (v2), `workflowId` (v2,
  nested workflows), or a non-`simple` `successCriteria.type` (v2,
  regex/jsonpath). Exit code 0; the runner will skip these constructs
  at execution time. Tracked in
  [issue #5](https://github.com/rustpunk/spall/issues/5).

```text
$ spall arazzo validate ./onboard.arazzo.yaml
ok: './onboard.arazzo.yaml' parses cleanly; all v1 features supported (1 workflow, 1 source).
```

## Dry-run

`--dry-run` parses the workflow, evaluates expressions, and prints each
step's resolved request (method, URL, headers, body) to stderr — but
sends nothing. Useful for sanity-checking expression bindings before a
real run hits an external system.

```text
$ spall arazzo run ./onboard.arazzo.yaml --input email=a@b.test --dry-run
[dry-run] step 'createUser': POST https://example.com/customers
            header: {"Content-Type": "application/json"}
            body: {"email":"a@b.test"}
```

## Failure actions

Step and workflow-level action chains let a workflow recover from
HTTP failures and unmet `successCriteria` per Arazzo §4.6 / §4.7.
Three action types are supported:

### `type: end`

Stops the workflow. Exit code depends on which side fired:

- Success-side `type:end` (in `onSuccess` or `successActions`) →
  exit 0 with workflow outputs.
- Failure-side `type:end` (in `onFailure` or `failureActions`) →
  exit non-zero with workflow + step attribution in stderr. The
  workflow took its failure branch and CI pipelines need to surface
  that.

```yaml
steps:
  - stepId: probe
    operationId: getProbeStatus
    onFailure:
      - name: known-4xx
        type: end
        criteria:
          - condition: $response.statusCode == 404
```

If no `criteria` are listed, the action fires unconditionally. To
absorb a known-OK 4xx into a zero exit, use `type: goto` to a
cleanup step (see below), not `type: end` on the failure side.

### `type: retry`

Sleeps for `retryAfter` seconds then re-runs the current step, up to
`retryLimit` times. When the limit is reached, the workflow exits
non-zero with the last error attached:

```yaml
steps:
  - stepId: callFlakyAPI
    operationId: getThing
    onFailure:
      - name: try-again
        type: retry
        retryAfter: 0.5
        retryLimit: 3
```

`retryLimit` defaults to `1` if omitted; `retryAfter` defaults to `0`.
The runner clamps each retry sleep at 60 seconds — a buggy spec with
`retryAfter: 999999` cannot hang the workflow indefinitely. The
retry counter does NOT compose with `--spall-retry` (the HTTP-transport
retry layer); they're orthogonal.

`type: retry` also fires on transport errors (DNS / connection-reset /
TLS handshake fails), not just HTTP 4xx/5xx — that's the exact case
backoff exists for.

### `type: goto`

Jumps to the named step. `workflowId` (cross-workflow goto) is a v2
feature and rejects at runtime if used:

```yaml
steps:
  - stepId: probe
    operationId: probe
    onFailure:
      - name: recover
        type: goto
        stepId: cleanupStep
  - stepId: shouldBeSkipped
    operationId: probe
  - stepId: cleanupStep
    operationId: cleanup
```

### Workflow-level fallback

`workflow.successActions` and `workflow.failureActions` apply to every
step that doesn't define its own `onSuccess` / `onFailure` chain.
Step-level absence vs explicit-empty matter:

- `onFailure` field absent on the step → workflow-level applies.
- `onFailure: []` on the step → opt out of the workflow-level default,
  no actions fire on failure (the underlying error bubbles up).
- `onFailure: [...]` non-empty → step-level wins; workflow-level is
  not consulted.

```yaml
workflows:
  - workflowId: paranoid
    failureActions:
      - name: bail
        type: end
    steps:
      - stepId: a
        operationId: a-op            # workflow-level 'bail' applies
      - stepId: b
        operationId: b-op
        onFailure: []                # opts out of workflow-level
```

### Reusable named actions

Heavy uses can centralize actions under `components`:

```yaml
components:
  failureActions:
    bail:
      name: bail
      type: end
workflows:
  - workflowId: x
    steps:
      - stepId: probe
        operationId: probe
        onFailure:
          - reference: $components.failureActions.bail
```

Reference paths must be exactly
`$components.successActions.<name>` or
`$components.failureActions.<name>` — typos error at workflow-start
time so a malformed reference doesn't silently fall through.

### Criterion `type`

Action `criteria` reuse the same condition mini-language as
`successCriteria`. v1 supports only `type: simple` (the default);
`jsonpath` and `regex` are deferred to issue #5 and error hard at
dispatch time so partial implementations don't sneak in via fixtures.
A non-empty `context` field — only used by v2 jsonpath/regex — also
errors hard so it can't be confused with simple-mode evaluation.

### Step budget

`spall arazzo run --spall-max-steps N` caps the total number of step
executions per workflow (default `10000`). The counter increments on
every step body run including retries and `goto`-revisits. A `goto X`
from step X with always-true criteria — the textbook infinite-loop
shape — bails with `StepBudgetExhausted` once the counter overshoots.

## v1 limitations

| Feature              | v1                  | Tracking         |
|----------------------|---------------------|------------------|
| `failureActions` / `successActions` (workflow + step level) | **Implemented** (this release) | — |
| `onSuccess` / `onFailure` actions | **Implemented** (this release) | — |
| `$components.successActions` / `$components.failureActions` refs | **Implemented** (this release) | — |
| `workflowId` (nested) | Errors at runtime  | issue #5         |
| `replay` action       | Not implemented    | issue #5         |
| `operationPath`       | Errors at runtime  | issue #5         |
| `successCriteria.type: regex` / `jsonpath` | Skipped with a warning | issue #5 |
| Action `criteria.type: regex` / `jsonpath` | Errors at dispatch | issue #5 |
| `--spall-bind <source>=<api>` CLI override | Use `x-spall-api` extension | issue #5 |
| Inputs JSON Schema validation | None — values are opaque strings | issue #5 |

Anything not in the table is in scope for v1.
