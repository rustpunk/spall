# Exit Codes

Spall returns structured exit codes so shell scripts and CI pipelines can branch on outcome without parsing stderr.

| Code | Meaning | When It Happens |
|------|---------|-----------------|
| 0 | Success | 2xx HTTP response |
| 1 | Usage error | Missing required argument, unknown API/operation, bad flag, config parse failure |
| 2 | Network error | DNS failure, TCP timeout, TLS error, proxy failure, stale cache with no fallback |
| 3 | Spec error | YAML/JSON parse failure, dangling `$ref`, invalid OpenAPI structure, cache corruption that cannot be rebuilt |
| 4 | HTTP 4xx | Client error responses (400, 401, 403, 404, etc.) |
| 5 | HTTP 5xx | Server error responses (500, 502, 503, etc.) |
| 10 | Validation failed | Preflight parameter or body schema validation failed |

## Scripting Examples

### Retry on network failure

```bash
spall petstore get-pet-by-id 1 || [ $? -eq 2 ] && sleep 5 && spall petstore get-pet-by-id 1
```

### Skip downstream steps on 4xx

```bash
spall github get-repo rustpunk/spall || {
  code=$?
  if [ "$code" -eq 4 ]; then
    echo "Repo not found, skipping build..."
    exit 0
  fi
  exit "$code"
}
```

### Fail CI on validation errors

```bash
spall internal create-order --data @order.json || {
  code=$?
  if [ "$code" -eq 10 ]; then
    echo "Validation failed — check your payload."
  fi
  exit "$code"
}
```

## Next Steps

- [Global Flags](../usage/global-flags.md)
- [CLI Reference](cli-reference.md)
