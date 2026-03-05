# Waz — Command prediction for Zsh
# Usage: eval "$(waz init zsh)"

# --- Session setup ---
export WAZ_SESSION_ID="${WAZ_SESSION_ID:-$(command waz session-id)}"

# --- Record commands after execution + capture output for hints ---
typeset -g _WAZ_LAST_CMD=""
typeset -g _WAZ_OUTPUT_FILE=""
typeset -g _WAZ_CAPTURING=0

_waz_preexec() {
    _WAZ_LAST_CMD="$1"
    _WAZ_OUTPUT_FILE=$(mktemp /tmp/waz_output.XXXXXX 2>/dev/null || echo "")
    if [[ -n "$_WAZ_OUTPUT_FILE" ]]; then
        # Save original stdout/stderr, then tee output to file
        exec 3>&1 4>&2
        exec 1> >(tee -a "$_WAZ_OUTPUT_FILE" >&3) 2> >(tee -a "$_WAZ_OUTPUT_FILE" >&4)
        _WAZ_CAPTURING=1
    fi
}

_waz_precmd() {
    local exit_code=$?

    # Restore stdout/stderr from capture
    if (( _WAZ_CAPTURING )); then
        exec 1>&3 2>&4 3>&- 4>&-
        _WAZ_CAPTURING=0
    fi

    if [[ -n "$_WAZ_LAST_CMD" ]]; then
        command waz record --cwd "$PWD" --session "$WAZ_SESSION_ID" --exit-code "$exit_code" -- "$_WAZ_LAST_CMD" &>/dev/null &!

        # Extract command hints from captured output
        if [[ -n "$_WAZ_OUTPUT_FILE" && -f "$_WAZ_OUTPUT_FILE" && -s "$_WAZ_OUTPUT_FILE" ]]; then
            local last_lines
            last_lines=$(tail -30 "$_WAZ_OUTPUT_FILE" 2>/dev/null)
            if [[ -n "$last_lines" ]]; then
                command waz hint --output "$last_lines" &>/dev/null &!
            fi
        fi
        [[ -f "$_WAZ_OUTPUT_FILE" ]] && rm -f "$_WAZ_OUTPUT_FILE" 2>/dev/null
        _WAZ_OUTPUT_FILE=""
        _WAZ_LAST_CMD=""
    fi
    _WAZ_SHOW_PROACTIVE=1
}

autoload -Uz add-zsh-hook
add-zsh-hook preexec _waz_preexec
add-zsh-hook precmd _waz_precmd

# --- Ghost text prediction via ZLE ---
typeset -g _WAZ_SUGGESTION=""
typeset -g _WAZ_SHOW_PROACTIVE=""

# Min chars before triggering a prediction (avoids spawning process for every letter)
typeset -g _WAZ_MIN_CHARS=2

_waz_suggest() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""
    region_highlight=("${(@)region_highlight:#*fg=#6c6c6c*}")

    local buf="$BUFFER"
    local prediction

    if [[ -z "${buf// /}" ]]; then
        # Empty buffer: proactive prediction (full mode w/ LLM)
        prediction=$(command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" 2>/dev/null)
    elif (( ${#buf} >= _WAZ_MIN_CHARS )); then
        # Fast local prediction only (no LLM, no blocking)
        prediction=$(command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" --prefix "$buf" --fast 2>/dev/null)
    else
        return
    fi

    if [[ -n "$prediction" && "$prediction" != "$buf" ]]; then
        _WAZ_SUGGESTION="$prediction"
        if [[ -z "${buf// /}" ]]; then
            POSTDISPLAY="$prediction"
        elif [[ "$prediction" == "$buf"* ]]; then
            POSTDISPLAY="${prediction#$buf}"
        else
            POSTDISPLAY="  ← $prediction"
        fi
        local start=${#BUFFER}
        local end=$((start + ${#POSTDISPLAY}))
        region_highlight+=("$start $end fg=#6c6c6c")
    fi
}

_waz_clear() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""
}

# --- Widget: show proactive suggestion when line editor starts ---
# Note: We chain with any existing zle-line-init (e.g. vi-mode cursor setup)
if (( ${+functions[zle-line-init]} )); then
    # Save existing zle-line-init and chain ours
    functions[_waz_original_line_init]=$functions[zle-line-init]
    _waz_line_init() {
        _waz_original_line_init "$@"
        if [[ -n "$_WAZ_SHOW_PROACTIVE" ]]; then
            _WAZ_SHOW_PROACTIVE=""
            _waz_suggest
        fi
    }
else
    _waz_line_init() {
        if [[ -n "$_WAZ_SHOW_PROACTIVE" ]]; then
            _WAZ_SHOW_PROACTIVE=""
            _waz_suggest
        fi
    }
fi

# --- Widget: self-insert + suggest ---
_waz_self_insert() {
    zle .self-insert
    _waz_suggest
}

# --- Widget: backward-delete + suggest ---
_waz_backward_delete() {
    zle .backward-delete-char
    _waz_suggest
}

# --- Widget: accept full suggestion (right arrow) ---
_waz_accept() {
    if [[ -n "$_WAZ_SUGGESTION" ]]; then
        BUFFER="$_WAZ_SUGGESTION"
        CURSOR=${#BUFFER}
        _waz_clear
    else
        zle .forward-char
    fi
}

# --- Widget: accept one word (alt+f) ---
_waz_accept_word() {
    if [[ -n "$_WAZ_SUGGESTION" && "$_WAZ_SUGGESTION" != "$BUFFER" ]]; then
        local remaining="${_WAZ_SUGGESTION#$BUFFER}"
        local next_word="${remaining%% *}"
        if [[ "$remaining" == "$next_word" ]]; then
            BUFFER="$_WAZ_SUGGESTION"
        else
            BUFFER="${BUFFER}${next_word} "
        fi
        CURSOR=${#BUFFER}
        _waz_suggest
    else
        zle .forward-word
    fi
}

# --- Widget: enter clears ghost text ---
_waz_accept_line() {
    _waz_clear
    zle .accept-line
}

# --- Widget: ctrl-c clears ghost text ---
_waz_send_break() {
    _waz_clear
    zle .send-break
}

# --- Register all widgets ---
zle -N self-insert _waz_self_insert
zle -N backward-delete-char _waz_backward_delete
zle -N _waz_accept
zle -N _waz_accept_word
zle -N accept-line _waz_accept_line
zle -N send-break _waz_send_break
zle -N zle-line-init _waz_line_init

# --- Keybindings ---
bindkey '^[[C' _waz_accept
bindkey '^[f'  _waz_accept_word

# --- Natural language via command_not_found_handler ---
command_not_found_handler() {
    local full_input="$*"

    if command waz check-nl -- $full_input 2>/dev/null; then
        # Don't record NL queries in command history
        _WAZ_LAST_CMD=""

        # Try TUI AI mode first (interactive, with selectable commands)
        local tui_result
        tui_result=$(command waz tui --cwd "$PWD" --query "$full_input" < /dev/tty 2>/dev/null)

        if [[ -n "$tui_result" ]]; then
            # Pre-fill the prompt so user can review before executing
            print -z "$tui_result"
            return 0
        fi

        # Fallback: inline JSON resolver (for non-interactive contexts)
        local json_resp
        json_resp=$(command waz ask --json --cwd "$PWD" --session "$WAZ_SESSION_ID" -- $full_input 2>/dev/null)

        if [[ -n "$json_resp" ]]; then
            _waz_interactive_resolver "$json_resp" "$full_input"
            return $?
        fi
    fi

    echo "zsh: command not found: $1"
    return 127
}

# --- Interactive command resolver ---
_waz_interactive_resolver() {
    local json="$1"
    local query="$2"

    # Parse JSON fields using waz's own JSON (no dependency on jq)
    local explanation
    explanation=$(echo "$json" | sed -n 's/.*"explanation":"\([^"]*\)".*/\1/p')

    # Print explanation
    echo ""
    echo "\033[0;33m🔮 waz:\033[0m"
    if [[ -n "$explanation" ]]; then
        echo "  $explanation"
        echo ""
    fi

    # Extract commands into arrays
    local -a cmds descs
    local -a placeholder_lists
    local i=0

    # Parse commands from JSON using simple pattern matching
    local remaining="$json"
    while [[ "$remaining" == *'"cmd":'* ]]; do
        # Extract cmd
        local cmd_part="${remaining#*\"cmd\":\"}"
        local cmd_val="${cmd_part%%\"*}"
        cmds+=("$cmd_val")

        # Extract desc
        local desc_part="${remaining#*\"desc\":\"}"
        local desc_val="${desc_part%%\"*}"
        descs+=("$desc_val")

        # Move past this command object
        remaining="${cmd_part#*\}}"
        i=$((i + 1))
    done

    local num_cmds=$i

    if (( num_cmds == 0 )); then
        # No commands — just show the explanation
        return 0
    fi

    # Display numbered menu
    for (( i=1; i<=num_cmds; i++ )); do
        local cmd="${cmds[$i]}"
        local desc="${descs[$i]}"
        if [[ -n "$desc" ]]; then
            printf "  \033[0;36m[%d]\033[0m %s  \033[0;90m— %s\033[0m\n" "$i" "$cmd" "$desc"
        else
            printf "  \033[0;36m[%d]\033[0m %s\n" "$i" "$cmd"
        fi
    done

    echo ""

    if (( num_cmds == 1 )); then
        _waz_resolve_command "${cmds[1]}"
    else
        # Multi-command menu
        echo -n "\033[0;90m  Pick command (1-$num_cmds), [a]ll, or [q]uit: \033[0m"
        local choice
        read -r choice

        case "$choice" in
            q|Q) return 0 ;;
            a|A)
                # Run all commands sequentially
                for (( i=1; i<=num_cmds; i++ )); do
                    _waz_resolve_command "${cmds[$i]}"
                done
                ;;
            *)
                if [[ "$choice" =~ ^[0-9]+$ ]] && (( choice >= 1 && choice <= num_cmds )); then
                    _waz_resolve_command "${cmds[$choice]}"
                else
                    echo "  Invalid choice."
                fi
                ;;
        esac
    fi
}

# --- Resolve a single command: fill placeholders, then run or pre-fill ---
_waz_resolve_command() {
    local cmd="$1"
    local resolved="$cmd"

    # Extract placeholders <...> from the command
    local -a placeholders
    local tmp="$cmd"
    while [[ "$tmp" == *"<"*">"* ]]; do
        local ph="${tmp#*<}"
        ph="${ph%%>*}"
        placeholders+=("$ph")
        tmp="${tmp#*>}"
    done

    if (( ${#placeholders} > 0 )); then
        echo ""
        echo "\033[0;90m  ⌨  Fill in placeholders:\033[0m"

        for ph in "${placeholders[@]}"; do
            local value=""
            # Use vared for file-related placeholders (enables tab completion)
            if [[ "$ph" == *file* || "$ph" == *path* || "$ph" == *dir* ]]; then
                echo -n "    $ph: "
                vared -c value
            else
                echo -n "    $ph: "
                read -r value
            fi

            if [[ -z "$value" ]]; then
                echo "  ⚠  Skipped — copied to prompt for editing."
                print -z "$resolved"
                return 0
            fi

            # Replace ALL occurrences of this placeholder
            resolved="${resolved//<$ph>/$value}"
        done
    fi

    echo ""
    echo "\033[0;32m  → $resolved\033[0m"
    echo ""
    echo -n "\033[0;90m  Run this command? [Y/n/e(dit)] \033[0m"
    local reply
    read -r reply

    case "$reply" in
        e|E)
            print -z "$resolved"
            ;;
        n|N)
            return 0
            ;;
        *)
            eval "$resolved"
            ;;
    esac
}

# --- TUI launcher widget ---
# Ctrl+T → Unified waz TUI (command palette + AI + shell)

_waz_tui() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""

    # Use a temp file to pass the selected command — avoids $() subshell
    # which breaks crossterm's stdin event reader.
    local tmpfile="${TMPDIR:-/tmp}/.waz_result.$$"

    # Run waz tui with ALL fds on /dev/tty (clean terminal, no subshell)
    command waz tui --cwd "$PWD" --result-file "$tmpfile" </dev/tty >/dev/tty 2>/dev/tty

    zle reset-prompt

    # Read the result from temp file
    if [[ -f "$tmpfile" ]]; then
        local result
        result=$(<"$tmpfile")
        rm -f "$tmpfile"

        if [[ -n "$result" ]]; then
            BUFFER="$result"
            CURSOR=${#BUFFER}
            zle accept-line
        fi
    fi
}

zle -N _waz_tui

# Bind Ctrl+T in ALL keymaps (main, viins, vicmd) — works in most terminals
bindkey '^T' _waz_tui
bindkey -M viins '^T' _waz_tui
bindkey -M vicmd '^T' _waz_tui

# Bind Cmd+I via Ghostty's custom escape sequence
bindkey '\e[119;97;122~' _waz_tui
bindkey -M viins '\e[119;97;122~' _waz_tui
bindkey -M vicmd '\e[119;97;122~' _waz_tui

# ─── Config mode (waz commands only) ───

_waz_config() {
    local tmpfile=$(mktemp /tmp/waz_result.XXXXXX)
    command waz tui --cwd "$PWD" --config --result-file "$tmpfile" </dev/tty >/dev/tty 2>/dev/tty
    zle reset-prompt
    if [[ -f "$tmpfile" ]]; then
        local result
        result=$(<"$tmpfile")
        rm -f "$tmpfile"
        if [[ -n "$result" ]]; then
            BUFFER="$result"
            CURSOR=${#BUFFER}
            zle accept-line
        fi
    fi
}

zle -N _waz_config

# Bind Cmd+Shift+I via Ghostty's custom escape sequence
bindkey '\e[119;97;99~' _waz_config
bindkey -M viins '\e[119;97;99~' _waz_config
bindkey -M vicmd '\e[119;97;99~' _waz_config
