set shell := ["bash", "-uc"]

default: check

fmt:
    @echo "[*] Formatting..."
    @cargo fmt
    @echo "[+] Formatted"

fmt-check:
    @echo "[*] Checking formatting..."
    @cargo fmt --check
    @echo "[+] Formatting OK"

clippy:
    @echo "[*] Running clippy..."
    @cargo clippy --all-targets --all-features --quiet -- -D warnings
    @echo "[+] Clippy passed"

test:
    @echo "[*] Running tests..."
    @cargo test --quiet
    @echo "[+] Tests passed"

check: fmt-check clippy test
    @echo ""
    @echo "[+] All checks passed!"

fix: fmt
    @echo "[*] Running clippy --fix..."
    @cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged --quiet -- -D warnings
    @echo "[+] Fixed"
