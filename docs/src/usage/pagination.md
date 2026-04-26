# Pagination

Many APIs paginate large result sets via `Link` headers (RFC 5988). Spall can auto-follow these links and concatenate the results into a single JSON array.

## Basic Usage

```bash
spall github list-repos --spall-paginate
```

Spall sends the initial request, inspects the `Link` header for `rel="next"`, and follows it until:

- No `next` link is present.
- The maximum page limit (default 100) is reached.
- A non-2xx response is returned (spall exits with the appropriate code).

## Result Concatenation

If every page is a JSON array, all elements are flattened into one array:

```json
// Page 1: [{"id":1},{"id":2}]
// Page 2: [{"id":3},{"id":4}]
// Output:   [{"id":1},{"id":2},{"id":3},{"id":4}]
```

If a page is not an array, it is wrapped as a single item:

```json
// Page 1: [{"id":1}]
// Page 2: {"meta": {...}}
// Output:   [{"id":1},{"meta": {...}}]
```

## Combining with Output and Filtering

Pagination works with all output modes and filtering:

```bash
spall github list-repos --spall-paginate --spall-output csv
spall github list-repos --spall-paginate --filter "[].full_name"
```

## Limitations

- `--spall-paginate` cannot be combined with `--form` (multipart uploads).
- Pagination requires JSON responses. If a page returns non-JSON, spall exits with a usage error.

## Next Steps

- [Response Output](response-output.md)
- [Global Flags](global-flags.md)
