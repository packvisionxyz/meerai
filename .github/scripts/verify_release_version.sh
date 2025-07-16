#!/bin/sh
set -eu

event_name="${EVENT_NAME:-}"
ref_name="${REF_NAME:-}"
input_tag="${INPUT_TAG:-}"
input_prerelease="${INPUT_PRERELEASE:-false}"
cargo_toml="${CARGO_TOML:-Cargo.toml}"

if [ "$event_name" = "workflow_dispatch" ]; then
    tag="$input_tag"
    prerelease="$input_prerelease"
else
    tag="$ref_name"
    prerelease="false"
    case "$tag" in
        *-*) prerelease="true" ;;
    esac
fi

if [ -z "$tag" ]; then
    printf '::error::could not resolve a release tag (event %s)\n' "$event_name" >&2
    exit 1
fi

if [ ! -f "$cargo_toml" ]; then
    printf '::error::cannot find %s\n' "$cargo_toml" >&2
    exit 1
fi

workspace_version=$(awk '
    /^\[workspace\.package\]/ { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && /^version[[:space:]]*=/ {
        gsub(/^version[[:space:]]*=[[:space:]]*"|"[[:space:]]*$/, "")
        print
        exit
    }
' "$cargo_toml")

if [ -z "$workspace_version" ]; then
    printf '::error::could not read version from [workspace.package] in %s\n' "$cargo_toml" >&2
    exit 1
fi

tag_version="${tag#v}"
tag_base="${tag_version%%-*}"
tag_base="${tag_base%%+*}"

if [ -z "$tag_base" ]; then
    printf '::error::release tag %s does not contain a version\n' "$tag" >&2
    exit 1
fi

if [ "$tag_base" != "$workspace_version" ]; then
    printf '::error::release tag %s has version %s but %s has %s. Bump [workspace.package] version or retag.\n' \
        "$tag" "$tag_base" "$cargo_toml" "$workspace_version" >&2
    exit 1
fi

if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'tag=%s\nprerelease=%s\n' "$tag" "$prerelease" >> "$GITHUB_OUTPUT"
fi

printf "Tag '%s' matches workspace version '%s' (prerelease: %s).\n" "$tag" "$workspace_version" "$prerelease"
