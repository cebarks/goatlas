fixtures: fixtures-base fixtures-pie fixtures-noinline

fixtures-base:
    #!/bin/sh
    set -eu
    export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
    if ! version=$(go version 2>&1); then
        printf '%s\n' "local Go toolchain required: $version" >&2
        exit 1
    fi
    mkdir -p target/fixtures
    printf '%s\n' "$version" > target/fixtures/go-version.txt
    cd tests/fixtures/app
    export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
    go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-m=2' -o ../../../target/fixtures/app.unstripped . 2> ../../../target/fixtures/unstripped.compiler.log
    go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-m=2' -ldflags='-s -w' -o ../../../target/fixtures/app.stripped . 2> ../../../target/fixtures/stripped.compiler.log

fixtures-pie:
    #!/bin/sh
    set -eu
    export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
    go version
    mkdir -p target/fixtures
    cd tests/fixtures/app
    export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
    go build -mod=readonly -trimpath -buildvcs=false -buildmode=pie -ldflags='-s -w' -o ../../../target/fixtures/app.pie.stripped .

# app.noinline compiles the fixture package with inlining disabled so its
# own helpers stay standalone and their inline trees are genuinely empty.
fixtures-noinline:
    #!/bin/sh
    set -eu
    export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
    go version
    mkdir -p target/fixtures
    cd tests/fixtures/app
    export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
    go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-l' -ldflags='-s -w' -o ../../../target/fixtures/app.noinline . 2> ../../../target/fixtures/noinline.compiler.log

test: fixtures
    cargo test --locked

patch-check:
    #!/bin/sh
    set -eu
    export CARGO_TARGET_DIR="{{justfile_directory()}}/target/vendor-unstrip"
    cargo fmt --manifest-path vendor/unstrip/Cargo.toml --all -- --check
    cargo test --manifest-path vendor/unstrip/Cargo.toml --locked --lib
    cargo clippy --manifest-path vendor/unstrip/Cargo.toml --locked --lib -- -D warnings

check: test patch-check
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
