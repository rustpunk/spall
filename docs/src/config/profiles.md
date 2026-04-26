# Profiles

Profiles let you switch between environments (staging, production, local) without re-registering the API or editing files before every command.

## Defining Profiles

Add a `[profiles.{name}]` section to any per-API config file:

```toml
# ~/.config/spall/apis/petstore.toml
source = "https://petstore.swagger.io/v2/swagger.json"
base_url = "https://petstore.io"

[auth]
kind = "Bearer"
token_env = "PETSTORE_TOKEN"

[profiles.staging]
base_url = "https://staging.petstore.io"
auth = { kind = "Bearer", token_env = "PETSTORE_STAGING_TOKEN" }

[profiles.production]
base_url = "https://petstore.io"
```

## Using Profiles

Activate a profile with `--profile`:

```bash
# Hit staging
spall petstore get-pet-by-id 1 --profile staging

# Hit production
spall petstore get-pet-by-id 1 --profile production
```

## Profile Overlay Rules

When a profile is active, its values override the base config:

- `base_url` replaces the base config value entirely.
- `auth` replaces the base config auth entirely.
- `headers` are merged: profile headers with the same key override base headers; new keys are appended.

If the requested profile does not exist, spall exits with a usage error.

## Next Steps

- [Config Layout](layout.md)
- [Authentication](../usage/authentication.md)
