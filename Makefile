.DEFAULT_GOAL := help
MYSYNC_CARGO_JOBS ?= 1
.PHONY: help test build install test-tpm clean
help:
	@printf '%s\n' 'make test: client, installer, profiles and signed updates without a TPM driver' 'make build: production client with TPM support' 'make test-tpm: TPM driver tests; cross-repository suite lives in mysyncfiles-server' 'make install: install the built client and enable its user service'
test:
	sh -n deploy/install.sh
	bash -n deploy/install-client.sh
	cargo fmt --all -- --check
	cargo test --locked --no-default-features --jobs $(MYSYNC_CARGO_JOBS)
build:
	cargo build --locked --release --features tpm --jobs $(MYSYNC_CARGO_JOBS)
test-tpm:
	cargo test --locked --features tpm --jobs $(MYSYNC_CARGO_JOBS)
install:
	bash deploy/install-client.sh
clean:
	cargo clean
