#!/usr/bin/env bash
# validate_tmp_schema.sh — Validate TMP (Token Model Protocol) schema files
#
# Usage:
#   script/validate_tmp_schema.sh <schema.json> [OPTIONS]
#
# Options:
#   --help                Show this help message
#   --verbose             Show detailed output
#   --meta-schema <path>  Path to meta-schema (default: resources/tmp/meta-schema.json)
#   --check-help          Compare schema commands against <tool> --help output
#   --verify-sources      Try running data_source commands and report empty results

set -euo pipefail

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

# --- Globals ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
META_SCHEMA="${REPO_ROOT}/resources/tmp/meta-schema.json"
VERBOSE=false
CHECK_HELP=false
VERIFY_SOURCES=false
SCHEMA_FILE=""

ERRORS=()
WARNINGS=()
PASSES=()

# --- Functions ---

usage() {
    cat <<EOF
${BOLD}validate_tmp_schema.sh${RESET} — Validate TMP schema files

${BOLD}USAGE:${RESET}
    script/validate_tmp_schema.sh <schema.json> [OPTIONS]

${BOLD}OPTIONS:${RESET}
    --help                Show this help message
    --verbose             Show detailed output
    --meta-schema <path>  Path to meta-schema JSON file
                          (default: resources/tmp/meta-schema.json)
    --check-help          Compare schema commands against <tool> --help output
    --verify-sources      Try running data_source commands and report failures

${BOLD}EXIT CODES:${RESET}
    0  PASS — All checks passed
    1  FAIL — Critical errors found
    2  WARN — Non-critical warnings found
EOF
}

log_pass() {
    PASSES+=("$1")
    if $VERBOSE; then
        echo -e "  ${GREEN}✓${RESET} $1"
    fi
}

log_warn() {
    WARNINGS+=("$1")
    echo -e "  ${YELLOW}⚠${RESET} $1"
}

log_error() {
    ERRORS+=("$1")
    echo -e "  ${RED}✗${RESET} $1"
}

log_info() {
    if $VERBOSE; then
        echo -e "  ${CYAN}ℹ${RESET} $1"
    fi
}

# --- Argument Parsing ---

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --help|-h)
                usage
                exit 0
                ;;
            --verbose|-v)
                VERBOSE=true
                shift
                ;;
            --meta-schema)
                if [[ -z "${2:-}" ]]; then
                    echo -e "${RED}Error:${RESET} --meta-schema requires a path argument"
                    exit 1
                fi
                META_SCHEMA="$2"
                shift 2
                ;;
            --check-help)
                CHECK_HELP=true
                shift
                ;;
            --verify-sources)
                VERIFY_SOURCES=true
                shift
                ;;
            -*)
                echo -e "${RED}Error:${RESET} Unknown option: $1"
                usage
                exit 1
                ;;
            *)
                if [[ -z "$SCHEMA_FILE" ]]; then
                    SCHEMA_FILE="$1"
                else
                    echo -e "${RED}Error:${RESET} Multiple schema files specified"
                    exit 1
                fi
                shift
                ;;
        esac
    done

    if [[ -z "$SCHEMA_FILE" ]]; then
        echo -e "${RED}Error:${RESET} No schema file specified"
        echo ""
        usage
        exit 1
    fi

    if [[ ! -f "$SCHEMA_FILE" ]]; then
        echo -e "${RED}Error:${RESET} File not found: $SCHEMA_FILE"
        exit 1
    fi
}

# --- Check: jq is available ---

check_prerequisites() {
    if ! command -v jq &>/dev/null; then
        echo -e "${RED}Error:${RESET} jq is required but not found. Install it with:"
        echo "  brew install jq       # macOS"
        echo "  apt install jq        # Debian/Ubuntu"
        exit 1
    fi
}

# --- Check 1: Valid JSON ---

check_json_validity() {
    echo -e "\n${BOLD}[1/6] Structural Validation${RESET}"

    if jq empty "$SCHEMA_FILE" 2>/dev/null; then
        log_pass "Valid JSON"
    else
        log_error "Invalid JSON — file cannot be parsed"
        return
    fi

    # Check top-level keys
    local has_meta has_commands
    has_meta=$(jq 'has("meta")' "$SCHEMA_FILE")
    has_commands=$(jq 'has("commands")' "$SCHEMA_FILE")

    if [[ "$has_meta" == "true" ]]; then
        log_pass "Top-level 'meta' field present"
    else
        log_error "Missing required top-level field: 'meta'"
    fi

    if [[ "$has_commands" == "true" ]]; then
        log_pass "Top-level 'commands' field present"
    else
        log_error "Missing required top-level field: 'commands'"
        return
    fi

    # Check commands is non-empty array
    local cmd_count
    cmd_count=$(jq '.commands | length' "$SCHEMA_FILE")
    if [[ "$cmd_count" -gt 0 ]]; then
        log_pass "Commands array has $cmd_count command(s)"
    else
        log_error "Commands array is empty (must have at least 1 command)"
    fi

    # Check meta.tool
    local tool
    tool=$(jq -r '.meta.tool // empty' "$SCHEMA_FILE")
    if [[ -n "$tool" ]]; then
        log_pass "meta.tool = '$tool'"
    else
        log_error "Missing required field: meta.tool"
    fi
}

# --- Check 2: Meta-Schema Validation ---

check_meta_schema() {
    echo -e "\n${BOLD}[2/6] Meta-Schema Validation${RESET}"

    if ! [[ -f "$META_SCHEMA" ]]; then
        log_warn "Meta-schema not found at: $META_SCHEMA (skipping jsonschema validation)"
        run_manual_meta_checks
        return
    fi

    # Try jsonschema CLI (Python: pip install jsonschema)
    if command -v jsonschema &>/dev/null; then
        log_info "Using 'jsonschema' CLI for validation"
        local output
        if output=$(jsonschema -i "$SCHEMA_FILE" "$META_SCHEMA" 2>&1); then
            log_pass "Meta-schema validation passed (jsonschema CLI)"
        else
            log_error "Meta-schema validation failed:"
            while IFS= read -r line; do
                log_error "  $line"
            done <<< "$output"
        fi
    # Try check-jsonschema (Python: pip install check-jsonschema)
    elif command -v check-jsonschema &>/dev/null; then
        log_info "Using 'check-jsonschema' CLI for validation"
        local output
        if output=$(check-jsonschema --schemafile "$META_SCHEMA" "$SCHEMA_FILE" 2>&1); then
            log_pass "Meta-schema validation passed (check-jsonschema CLI)"
        else
            log_error "Meta-schema validation failed:"
            while IFS= read -r line; do
                log_error "  $line"
            done <<< "$output"
        fi
    else
        log_info "No jsonschema CLI found, falling back to manual jq checks"
        run_manual_meta_checks
    fi
}

run_manual_meta_checks() {
    log_info "Running manual meta field checks with jq"

    # meta.schema_version
    local sv
    sv=$(jq '.meta.schema_version // null' "$SCHEMA_FILE")
    if [[ "$sv" != "null" ]]; then
        if [[ "$sv" == "1" || "$sv" == "2" ]]; then
            log_pass "meta.schema_version = $sv (valid)"
        else
            log_warn "meta.schema_version = $sv (expected 1 or 2)"
        fi
    fi

    # meta.coverage
    local cov
    cov=$(jq -r '.meta.coverage // empty' "$SCHEMA_FILE")
    if [[ -n "$cov" ]]; then
        if [[ "$cov" == "partial" || "$cov" == "full" ]]; then
            log_pass "meta.coverage = '$cov' (valid)"
        else
            log_error "meta.coverage = '$cov' (must be 'partial' or 'full')"
        fi
    fi

    # meta.discovery_method
    local dm
    dm=$(jq -r '.meta.discovery_method // empty' "$SCHEMA_FILE")
    if [[ -n "$dm" ]]; then
        if [[ "$dm" == "help" || "$dm" == "man" || "$dm" == "none" ]]; then
            log_pass "meta.discovery_method = '$dm' (valid)"
        else
            log_warn "meta.discovery_method = '$dm' (expected 'help', 'man', or 'none')"
        fi
    fi

    # Check each command has required fields
    local cmd_count
    cmd_count=$(jq '.commands | length' "$SCHEMA_FILE")
    for ((i = 0; i < cmd_count; i++)); do
        local cmd_name
        cmd_name=$(jq -r ".commands[$i].command // empty" "$SCHEMA_FILE")
        local cmd_desc
        cmd_desc=$(jq -r ".commands[$i].description // empty" "$SCHEMA_FILE")
        local cmd_group
        cmd_group=$(jq -r ".commands[$i].group // empty" "$SCHEMA_FILE")

        if [[ -z "$cmd_name" ]]; then
            log_error "commands[$i]: missing required field 'command'"
        fi
        if [[ -z "$cmd_desc" ]]; then
            log_error "commands[$i]: missing required field 'description'"
        fi
        if [[ -z "$cmd_group" ]]; then
            log_error "commands[$i]: missing required field 'group'"
        fi
    done
}

# --- Check 3: Type Correctness ---

check_type_correctness() {
    echo -e "\n${BOLD}[3/6] Type Correctness${RESET}"

    local cmd_count
    cmd_count=$(jq '.commands | length' "$SCHEMA_FILE")

    local known_resolvers="cargo:bins cargo:examples cargo:packages cargo:features cargo:profiles cargo:tests cargo:benches git:branches git:remotes git:status_files git:tags npm:scripts"

    for ((i = 0; i < cmd_count; i++)); do
        local cmd_name
        cmd_name=$(jq -r ".commands[$i].command" "$SCHEMA_FILE")
        local token_count
        token_count=$(jq ".commands[$i].tokens // [] | length" "$SCHEMA_FILE")

        for ((j = 0; j < token_count; j++)); do
            local token_path="commands[$i].tokens[$j]"
            local token_name token_type
            token_name=$(jq -r ".commands[$i].tokens[$j].name // empty" "$SCHEMA_FILE")
            token_type=$(jq -r ".commands[$i].tokens[$j].token_type // empty" "$SCHEMA_FILE")

            local display_name="${token_name:-$token_path}"

            # Required token fields
            if [[ -z "$token_name" ]]; then
                log_error "$token_path: missing required field 'name'"
            fi
            if [[ -z "$token_type" ]]; then
                log_error "$token_path ($display_name): missing required field 'token_type'"
                continue
            fi

            # token_type enum check
            case "$token_type" in
                String|Boolean|Enum|File|Number)
                    log_pass "$display_name: token_type '$token_type' is valid"
                    ;;
                *)
                    log_error "$display_name: invalid token_type '$token_type' (must be String|Boolean|Enum|File|Number)"
                    continue
                    ;;
            esac

            # Boolean must NOT have values
            if [[ "$token_type" == "Boolean" ]]; then
                local has_values
                has_values=$(jq ".commands[$i].tokens[$j] | has(\"values\")" "$SCHEMA_FILE")
                if [[ "$has_values" == "true" ]]; then
                    local values_len
                    values_len=$(jq ".commands[$i].tokens[$j].values | length" "$SCHEMA_FILE")
                    if [[ "$values_len" -gt 0 ]]; then
                        log_error "$display_name: Boolean token should not have 'values'"
                    fi
                fi
            fi

            # Enum must have values or data_source
            if [[ "$token_type" == "Enum" ]]; then
                local has_values has_ds
                has_values=$(jq ".commands[$i].tokens[$j] | has(\"values\")" "$SCHEMA_FILE")
                has_ds=$(jq ".commands[$i].tokens[$j] | has(\"data_source\")" "$SCHEMA_FILE")
                if [[ "$has_values" != "true" && "$has_ds" != "true" ]]; then
                    log_warn "$display_name: Enum token should have 'values' or 'data_source'"
                else
                    log_pass "$display_name: Enum token has values/data_source"
                fi
            fi

            # Flag format check
            local flag
            flag=$(jq -r ".commands[$i].tokens[$j].flag // empty" "$SCHEMA_FILE")
            if [[ -n "$flag" ]]; then
                if [[ "$flag" =~ ^-{1,2}[a-zA-Z][a-zA-Z0-9-]*$ ]]; then
                    log_pass "$display_name: flag '$flag' format is valid"
                else
                    log_error "$display_name: flag '$flag' has invalid format (expected: -x or --long-flag)"
                fi
            fi

            # Aliases format check
            local alias_count
            alias_count=$(jq ".commands[$i].tokens[$j].aliases // [] | length" "$SCHEMA_FILE")
            for ((k = 0; k < alias_count; k++)); do
                local alias_val
                alias_val=$(jq -r ".commands[$i].tokens[$j].aliases[$k]" "$SCHEMA_FILE")
                if [[ ! "$alias_val" =~ ^-{1,2}[a-zA-Z][a-zA-Z0-9-]*$ ]]; then
                    log_error "$display_name: alias '$alias_val' has invalid format"
                fi
            done

            # Resolver check
            local resolver
            resolver=$(jq -r ".commands[$i].tokens[$j].data_source.resolver // empty" "$SCHEMA_FILE")
            if [[ -n "$resolver" ]]; then
                local found=false
                for r in $known_resolvers; do
                    if [[ "$r" == "$resolver" ]]; then
                        found=true
                        break
                    fi
                done
                if $found; then
                    log_pass "$display_name: resolver '$resolver' is known"
                else
                    log_error "$display_name: unknown resolver '$resolver'"
                fi
            fi

            # Fallback resolver check
            local fb_resolver
            fb_resolver=$(jq -r ".commands[$i].tokens[$j].data_source.fallback.resolver // empty" "$SCHEMA_FILE")
            if [[ -n "$fb_resolver" ]]; then
                local found=false
                for r in $known_resolvers; do
                    if [[ "$r" == "$fb_resolver" ]]; then
                        found=true
                        break
                    fi
                done
                if $found; then
                    log_pass "$display_name: fallback resolver '$fb_resolver' is known"
                else
                    log_error "$display_name: unknown fallback resolver '$fb_resolver'"
                fi
            fi

            # Parse field check
            local parse_val
            parse_val=$(jq -r ".commands[$i].tokens[$j].data_source.parse // empty" "$SCHEMA_FILE")
            if [[ -n "$parse_val" && "$parse_val" != "lines" && "$parse_val" != "words" ]]; then
                log_error "$display_name: data_source.parse = '$parse_val' (must be 'lines' or 'words')"
            fi
        done
    done
}

# --- Check 4: Completeness (--check-help) ---

check_help_completeness() {
    echo -e "\n${BOLD}[4/6] Completeness Check (--check-help)${RESET}"

    if ! $CHECK_HELP; then
        log_info "Skipped (use --check-help to enable)"
        if $VERBOSE; then :; else echo -e "  ${CYAN}ℹ${RESET} Skipped (use --check-help to enable)"; fi
        return
    fi

    local tool
    tool=$(jq -r '.meta.tool // empty' "$SCHEMA_FILE")
    if [[ -z "$tool" ]]; then
        log_warn "Cannot check help: meta.tool is empty"
        return
    fi

    if ! command -v "$tool" &>/dev/null; then
        log_warn "Tool '$tool' not found on PATH — cannot compare against --help"
        return
    fi

    log_info "Running: $tool --help"

    local help_output
    help_output=$("$tool" --help 2>&1 || true)

    # Extract subcommands from help output (heuristic: lines after "COMMANDS:" or "SUBCOMMANDS:")
    local help_commands=()
    local in_commands=false
    while IFS= read -r line; do
        # Detect command section headers
        if echo "$line" | grep -qiE '^\s*(commands|subcommands|available commands):?\s*$'; then
            in_commands=true
            continue
        fi
        # End of command section: empty line or new section header
        if $in_commands; then
            if [[ -z "$line" ]] || echo "$line" | grep -qE '^\s*[A-Z][A-Z ]+:'; then
                in_commands=false
                continue
            fi
            # Extract the first word (command name)
            local subcmd
            subcmd=$(echo "$line" | sed 's/^[[:space:]]*//' | awk '{print $1}')
            if [[ -n "$subcmd" && "$subcmd" != "-"* ]]; then
                help_commands+=("$subcmd")
            fi
        fi
    done <<< "$help_output"

    if [[ ${#help_commands[@]} -eq 0 ]]; then
        log_warn "Could not extract subcommands from '$tool --help' output"
        return
    fi

    log_info "Found ${#help_commands[@]} subcommand(s) in --help output"

    # Extract schema commands (first word after tool name)
    local schema_commands=()
    local cmd_count
    cmd_count=$(jq '.commands | length' "$SCHEMA_FILE")
    for ((i = 0; i < cmd_count; i++)); do
        local full_cmd
        full_cmd=$(jq -r ".commands[$i].command" "$SCHEMA_FILE")
        # Strip the tool name prefix and get the subcommand
        local subcmd
        subcmd=$(echo "$full_cmd" | sed "s/^${tool}[[:space:]]*//" | awk '{print $1}')
        if [[ -n "$subcmd" ]]; then
            schema_commands+=("$subcmd")
        fi
    done

    # Compare
    local missing=0
    for hcmd in "${help_commands[@]}"; do
        local found=false
        for scmd in "${schema_commands[@]}"; do
            if [[ "$hcmd" == "$scmd" ]]; then
                found=true
                break
            fi
        done
        if ! $found; then
            log_warn "Subcommand '$hcmd' found in --help but missing from schema"
            ((missing++))
        fi
    done

    if [[ $missing -eq 0 ]]; then
        log_pass "All --help subcommands are covered in the schema"
    else
        log_warn "$missing subcommand(s) from --help not found in schema"
    fi
}

# --- Check 5: Data Source Verification (--verify-sources) ---

check_data_sources() {
    echo -e "\n${BOLD}[5/6] Data Source Verification (--verify-sources)${RESET}"

    if ! $VERIFY_SOURCES; then
        log_info "Skipped (use --verify-sources to enable)"
        if $VERBOSE; then :; else echo -e "  ${CYAN}ℹ${RESET} Skipped (use --verify-sources to enable)"; fi
        return
    fi

    local cmd_count
    cmd_count=$(jq '.commands | length' "$SCHEMA_FILE")

    local checked=0
    for ((i = 0; i < cmd_count; i++)); do
        local token_count
        token_count=$(jq ".commands[$i].tokens // [] | length" "$SCHEMA_FILE")

        for ((j = 0; j < token_count; j++)); do
            local ds_cmd
            ds_cmd=$(jq -r ".commands[$i].tokens[$j].data_source.command // empty" "$SCHEMA_FILE")
            local token_name
            token_name=$(jq -r ".commands[$i].tokens[$j].name" "$SCHEMA_FILE")

            if [[ -n "$ds_cmd" ]]; then
                ((checked++))
                log_info "Testing data_source command for '$token_name': $ds_cmd"
                local output
                if output=$(eval "$ds_cmd" 2>/dev/null); then
                    if [[ -z "$output" ]]; then
                        log_warn "$token_name: data_source command returned empty results: $ds_cmd"
                    else
                        local line_count
                        line_count=$(echo "$output" | wc -l | tr -d ' ')
                        log_pass "$token_name: data_source command returned $line_count result(s)"
                    fi
                else
                    log_warn "$token_name: data_source command failed: $ds_cmd"
                fi
            fi

            # Also check fallback
            local fb_cmd
            fb_cmd=$(jq -r ".commands[$i].tokens[$j].data_source.fallback.command // empty" "$SCHEMA_FILE")
            if [[ -n "$fb_cmd" ]]; then
                ((checked++))
                log_info "Testing fallback command for '$token_name': $fb_cmd"
                local output
                if output=$(eval "$fb_cmd" 2>/dev/null); then
                    if [[ -z "$output" ]]; then
                        log_warn "$token_name: fallback command returned empty results: $fb_cmd"
                    else
                        log_pass "$token_name: fallback command works"
                    fi
                else
                    log_warn "$token_name: fallback command failed: $fb_cmd"
                fi
            fi
        done
    done

    if [[ $checked -eq 0 ]]; then
        log_info "No command-based data_sources found to verify"
        if ! $VERBOSE; then echo -e "  ${CYAN}ℹ${RESET} No command-based data_sources found to verify"; fi
    fi
}

# --- Check 6: Summary ---

print_summary() {
    echo -e "\n${BOLD}[6/6] Summary${RESET}"

    local pass_count=${#PASSES[@]}
    local warn_count=${#WARNINGS[@]}
    local error_count=${#ERRORS[@]}

    echo -e "  Checks passed:  ${GREEN}${pass_count}${RESET}"
    echo -e "  Warnings:       ${YELLOW}${warn_count}${RESET}"
    echo -e "  Errors:         ${RED}${error_count}${RESET}"
    echo ""

    if [[ $error_count -gt 0 ]]; then
        echo -e "  ${RED}${BOLD}FAIL${RESET} — $error_count error(s) found"
        if $VERBOSE; then
            echo ""
            echo -e "  ${RED}Errors:${RESET}"
            for err in "${ERRORS[@]}"; do
                echo -e "    ${RED}✗${RESET} $err"
            done
        fi
        return 1
    elif [[ $warn_count -gt 0 ]]; then
        echo -e "  ${YELLOW}${BOLD}WARN${RESET} — $warn_count warning(s) found"
        if $VERBOSE; then
            echo ""
            echo -e "  ${YELLOW}Warnings:${RESET}"
            for w in "${WARNINGS[@]}"; do
                echo -e "    ${YELLOW}⚠${RESET} $w"
            done
        fi
        return 2
    else
        echo -e "  ${GREEN}${BOLD}PASS${RESET} — All checks passed"
        return 0
    fi
}

# --- Main ---

main() {
    parse_args "$@"
    check_prerequisites

    echo -e "${BOLD}Validating TMP schema:${RESET} $SCHEMA_FILE"

    check_json_validity
    check_meta_schema
    check_type_correctness
    check_help_completeness
    check_data_sources

    print_summary
    exit $?
}

main "$@"
