# Request Bodies

When an operation declares a request body, spall adds `--data`, `--form`, and `--field` arguments. They are mutually exclusive.

## JSON Body with `--data`

The default for JSON APIs:

```bash
spall petstore add-pet --data '{"name":"Rex","status":"available"}'
```

### From a file

Prefix the path with `@`:

```bash
spall petstore add-pet --data @new-pet.json
```

### From stdin

Use `-`:

```bash
cat new-pet.json | spall petstore add-pet --data -
```

### Optional body

If the spec says the body is not required, spall also registers `--no-data` so you can skip it explicitly:

```bash
spall petstore update-pet --no-data
```

### Content-Type override

If the spec supports multiple content types, spall defaults to `application/json`. Override with `--spall-content-type`:

```bash
spall petstore upload-spec --data @spec.yaml --spall-content-type text/yaml
```

## Multipart Upload with `--form`

For `multipart/form-data` uploads, use `--form` (repeatable):

```bash
spall petstore upload-file --form file=@image.png --form description="avatar"
```

File values are auto-detected by the `@` prefix and streamed as binary parts.

## URL-Encoded with `--field`

For `application/x-www-form-urlencoded`, use `--field` (repeatable):

```bash
spall oauth token --field grant_type=client_credentials --field client_id=abc
```

## Validation

Before the request is sent, spall validates the body against the operation's request schema (when `application/json` is declared). Validation errors are printed to stderr and spall exits with code `10`.

```bash
spall petstore add-pet --data '{"status":"invalid"}'
# Validation failed:
#   /body/name: required property 'name' is missing
```

## Next Steps

- [Response Output](response-output.md)
- [Authentication](authentication.md)
