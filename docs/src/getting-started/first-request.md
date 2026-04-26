# Your First Request

This walkthrough registers the classic Petstore API, explores its operations, and makes a few requests.

## 1. Register the API

```bash
spall api add petstore https://petstore.swagger.io/v2/swagger.json
```

Spall will fetch the spec, cache it locally, and build an internal index. You only need to do this once.

## 2. List registered APIs

```bash
spall api list
```

Output:

```text
Registered APIs:
  petstore             https://petstore.swagger.io/v2/swagger.json
```

## 3. Explore operations

```bash
spall petstore --help
```

Spall loads the spec and prints every operation, grouped by OpenAPI tags. Operation names are derived from `operationId` (kebab-cased) or synthesized from the HTTP method and path.

## 4. Make a GET request

Path parameters become positional arguments. Query parameters become `--flags`.

```bash
# GET /pet/{petId}
spall petstore get-pet-by-id 1
```

If the terminal is a TTY, the response is pretty-printed JSON with syntax highlighting. If piped, raw JSON is emitted.

## 5. Make a POST request

Post a JSON body with `--data`:

```bash
spall petstore add-pet --data '{"name":"Rex","status":"available"}'
```

You can also read body data from a file or stdin:

```bash
spall petstore add-pet --data @new-pet.json
# or
cat new-pet.json | spall petstore add-pet --data -
```

## 6. Override the server

Point the same operation at a staging server without re-registering:

```bash
spall petstore get-pet-by-id 1 --spall-server https://staging.petstore.io
```

## 7. Dry-run and preview

See what spall will send without hitting the network:

```bash
spall petstore get-pet-by-id 1 --spall-dry-run
```

Or preview the fully resolved request (URL, headers, body):

```bash
spall petstore add-pet --data '{"name":"Rex"}' --spall-preview
```

## What just happened

1. **Phase 1** — spall scanned your config registry and matched `petstore` as a registered API name.
2. **Phase 2** — spall loaded the cached spec, resolved all `$ref`s, merged parameters, and built a dynamic clap command tree.
3. **Execution** — spall validated your inputs against the schema, built the HTTP request, sent it, and formatted the response.

## Next Steps

- [Key Concepts](concepts.md) — understand the mental model behind spall.
- [Registering APIs](../usage/registering-apis.md) — deep dive on API management.
- [Making Requests](../usage/making-requests.md) — all the ways to call an operation.
