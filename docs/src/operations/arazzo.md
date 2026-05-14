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

## v1 limitations

| Feature              | v1                  | Tracking         |
|----------------------|---------------------|------------------|
| `failureActions`     | Not honored         | issue #5         |
| `onSuccess` / `onFailure` actions | Not honored | issue #5         |
| `workflowId` (nested) | Errors at runtime  | issue #5         |
| `replay` action       | Not implemented    | issue #5         |
| `operationPath`       | Errors at runtime  | issue #5         |
| `successCriteria.type: regex` / `jsonpath` | Skipped with a warning | issue #5 |
| `--spall-bind <source>=<api>` CLI override | Use `x-spall-api` extension | issue #5 |
| Inputs JSON Schema validation | None — values are opaque strings | issue #5 |

Anything not in the table is in scope for v1.
