# Waz — Command prediction for Zsh
# Usage: eval "$(waz init zsh)"

# --- Session setup ---
export WAZ_SESSION_ID="${WAZ_SESSION_ID:-$(command waz session-id)}"

# --- Record commands after execution ---
_waz_preexec() {
    _WAZ_LAST_CMD="$1"
}

_waz_precmd() {
    local exit_code=$?
    if [[ -n "$_WAZ_LAST_CMD" ]]; then
        command waz record --cwd "$PWD" --session "$WAZ_SESSION_ID" --exit-code "$exit_code" -- "$_WAZ_LAST_CMD" &>/dev/null &!
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
_waz_line_init() {
    if [[ -n "$_WAZ_SHOW_PROACTIVE" ]]; then
        _WAZ_SHOW_PROACTIVE=""
        _waz_suggest
    fi
}

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
        tui_result=$(command waz tui ai --cwd "$PWD" --query "$full_input" 2>/dev/null)

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

# --- TUI launcher widgets ---
# `/` at empty prompt → TMP command mode
# `!` at empty prompt → Shell history mode
_waz_tui_tmp() {
    if [[ -z "$BUFFER" ]]; then
        # Clear suggestion/ghost text
        _WAZ_SUGGESTION=""
        POSTDISPLAY=""

        # Launch TUI — it takes over the terminal via alternate screen
        local result
        result=$(command waz tui tmp --cwd "$PWD" 2>/dev/null)

        # Re-init the ZLE line
        zle reset-prompt

        if [[ -n "$result" ]]; then
            # Pre-fill the command into the buffer for user to review/run
            BUFFER="$result"
            CURSOR=${#BUFFER}
        fi
    else
        # Normal `/` character when buffer is not empty
        BUFFER="${BUFFER}/"
        CURSOR=$((CURSOR + 1))
    fi
}

_waz_tui_shell() {
    if [[ -z "$BUFFER" ]]; then
        _WAZ_SUGGESTION=""
        POSTDISPLAY=""

        local result
        result=$(command waz tui shell --cwd "$PWD" 2>/dev/null)

        zle reset-prompt

        if [[ -n "$result" ]]; then
            BUFFER="$result"
            CURSOR=${#BUFFER}
        fi
    else
        BUFFER="${BUFFER}!"
        CURSOR=$((CURSOR + 1))
    fi
}

zle -N _waz_tui_tmp
zle -N _waz_tui_shell
bindkey '/' _waz_tui_tmp
bindkey '!' _waz_tui_shell

# Ctrl+A → AI mode TUI (works with or without buffer content)
_waz_tui_ai() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""

    local query_arg=""
    if [[ -n "$BUFFER" ]]; then
        query_arg="--query $BUFFER"
    fi

    local result
    result=$(eval "command waz tui ai --cwd \"$PWD\" $query_arg" 2>/dev/null)

    zle reset-prompt

    if [[ -n "$result" ]]; then
        BUFFER="$result"
        CURSOR=${#BUFFER}
    else
        BUFFER=""
        CURSOR=0
    fi
}

zle -N _waz_tui_ai
bindkey '^A' _waz_tui_ai
