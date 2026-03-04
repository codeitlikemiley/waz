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

# --- Async debounced predictions ---
# Architecture:
#   1. Each keystroke cancels pending prediction, starts 500ms bg timer
#   2. After 500ms, bg process runs `waz predict`, writes result to temp file
#   3. BG process sends SIGUSR1 to parent shell
#   4. TRAPUSR1 reads the file and updates POSTDISPLAY (ghost text)
#   5. zle -R redraws the line — all non-blocking, typing is never delayed

typeset -g _WAZ_SUGGESTION=""
typeset -g _WAZ_SHOW_PROACTIVE=""
typeset -g _WAZ_DEBOUNCE_PID=0
typeset -g _WAZ_RESULT_FILE="${TMPDIR:-/tmp}/waz_result_$$"

# --- Signal handler: async prediction result arrived ---
TRAPUSR1() {
    if [[ -f "$_WAZ_RESULT_FILE" ]]; then
        local pred
        pred=$(<"$_WAZ_RESULT_FILE")
        rm -f "$_WAZ_RESULT_FILE"

        if [[ -n "$pred" && "$pred" != "$BUFFER" ]]; then
            _WAZ_SUGGESTION="$pred"
            POSTDISPLAY=""
            region_highlight=("${(@)region_highlight:#*fg=#6c6c6c*}")

            local buf="$BUFFER"
            if [[ -z "${buf// /}" ]]; then
                POSTDISPLAY="$pred"
            elif [[ "$pred" == "$buf"* ]]; then
                POSTDISPLAY="${pred#$buf}"
            else
                POSTDISPLAY="  ← $pred"
            fi

            if [[ -n "$POSTDISPLAY" ]]; then
                local start=${#BUFFER}
                local end=$((start + ${#POSTDISPLAY}))
                region_highlight+=("$start $end fg=#6c6c6c")
            fi

            zle && zle -R
        fi
    fi
}

# --- Cancel pending prediction bg process ---
_waz_cancel_pending() {
    if (( _WAZ_DEBOUNCE_PID > 0 )); then
        kill $_WAZ_DEBOUNCE_PID 2>/dev/null
        _WAZ_DEBOUNCE_PID=0
    fi
    rm -f "$_WAZ_RESULT_FILE" 2>/dev/null
}

# --- Schedule a prediction (runs in bg after debounce delay) ---
_waz_schedule_predict() {
    local buf="$BUFFER"
    local cwd="$PWD"
    local sid="$WAZ_SESSION_ID"
    local result_file="$_WAZ_RESULT_FILE"
    local parent_pid=$$
    local is_proactive="$1"

    {
        # Debounce: wait 500ms (skip for proactive)
        [[ "$is_proactive" != "1" ]] && sleep 0.5

        local pred
        if [[ -n "${buf// /}" ]]; then
            pred=$(command waz predict --cwd "$cwd" --session "$sid" --prefix "$buf" --fast 2>/dev/null)
        else
            pred=$(command waz predict --cwd "$cwd" --session "$sid" 2>/dev/null)
        fi

        if [[ -n "$pred" ]]; then
            echo "$pred" > "$result_file"
            kill -USR1 $parent_pid 2>/dev/null
        fi
    } &!
    _WAZ_DEBOUNCE_PID=$!
}

# --- Clear ghost text ---
_waz_clear() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""
    region_highlight=("${(@)region_highlight:#*fg=#6c6c6c*}")
}

# --- Widget: proactive suggestion on new prompt ---
_waz_line_init() {
    if [[ -n "$_WAZ_SHOW_PROACTIVE" ]]; then
        _WAZ_SHOW_PROACTIVE=""
        _waz_cancel_pending
        _waz_schedule_predict 1
    fi
}

# --- Widget: self-insert (type a character) ---
_waz_self_insert() {
    zle .self-insert
    _waz_clear
    _waz_cancel_pending
    _waz_schedule_predict
}

# --- Widget: backward-delete (backspace) ---
_waz_backward_delete() {
    zle .backward-delete-char
    _waz_clear
    _waz_cancel_pending
    _waz_schedule_predict
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
        _waz_clear
        _waz_cancel_pending
        _waz_schedule_predict
    else
        zle .forward-word
    fi
}

# --- Widget: enter clears ghost text ---
_waz_accept_line() {
    _waz_clear
    _waz_cancel_pending
    zle .accept-line
}

# --- Widget: ctrl-c clears ghost text ---
_waz_send_break() {
    _waz_clear
    _waz_cancel_pending
    zle .send-break
}

# --- Cleanup on exit ---
_waz_cleanup() {
    _waz_cancel_pending
    rm -f "$_WAZ_RESULT_FILE" 2>/dev/null
}
add-zsh-hook zshexit _waz_cleanup

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
        local response
        response=$(command waz ask --cwd "$PWD" --session "$WAZ_SESSION_ID" -- $full_input 2>/dev/null)

        if [[ -n "$response" ]]; then
            local suggested_cmd=""
            local display_text=""

            if [[ "$response" == *"__WAZ_CMD__:"* ]]; then
                display_text="${response%%__WAZ_CMD__:*}"
                suggested_cmd="${response##*__WAZ_CMD__:}"
                suggested_cmd="${suggested_cmd%%$'\n'*}"
                suggested_cmd="${suggested_cmd## }"
            else
                display_text="$response"
            fi

            echo ""
            echo "\033[0;33m🔮 waz:\033[0m"
            echo "$display_text" | sed 's/^/  /'

            if [[ -n "$suggested_cmd" ]]; then
                echo ""
                echo "\033[0;32m  → $suggested_cmd\033[0m"
                echo ""
                echo -n "\033[0;90m  Run this command? [Y/n] \033[0m"
                read -r reply
                if [[ "$reply" =~ ^[Yy]?$ ]]; then
                    eval "$suggested_cmd"
                fi
            fi

            return 0
        fi
    fi

    echo "zsh: command not found: $1"
    return 127
}
