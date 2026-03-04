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

# --- Async prediction with debounce via FIFO + zle -F ---
typeset -g _WAZ_SUGGESTION=""
typeset -g _WAZ_SHOW_PROACTIVE=""
typeset -g _WAZ_DEBOUNCE_PID=0
typeset -g _WAZ_ASYNC_FD=""

# Setup async FIFO for non-blocking prediction results
_waz_setup_async() {
    local fifo="${TMPDIR:-/tmp}/waz_fifo_$$"
    [[ -p "$fifo" ]] && rm -f "$fifo"
    mkfifo "$fifo" 2>/dev/null || return

    # Open FIFO for read+write so we don't block
    exec {_WAZ_ASYNC_FD}<>"$fifo"
    rm -f "$fifo"  # Remove from filesystem; fd stays open

    # Register ZLE callback — fires when bg process writes to the FIFO
    zle -F $_WAZ_ASYNC_FD _waz_async_callback
}

# Called by ZLE when prediction result arrives on the FIFO
_waz_async_callback() {
    local prediction=""
    if ! read -r -u $1 prediction 2>/dev/null; then
        return
    fi
    [[ -z "$prediction" ]] && return

    # Clear old ghost text
    POSTDISPLAY=""
    region_highlight=("${(@)region_highlight:#*fg=#6c6c6c*}")

    local buf="$BUFFER"
    if [[ "$prediction" == "$buf" ]]; then
        return
    fi

    _WAZ_SUGGESTION="$prediction"

    if [[ -z "${buf// /}" ]]; then
        POSTDISPLAY="$prediction"
    elif [[ "$prediction" == "$buf"* ]]; then
        POSTDISPLAY="${prediction#$buf}"
    else
        POSTDISPLAY="  ← $prediction"
    fi

    if [[ -n "$POSTDISPLAY" ]]; then
        local start=${#BUFFER}
        local end=$((start + ${#POSTDISPLAY}))
        region_highlight+=("$start $end fg=#6c6c6c")
    fi

    zle -R  # Redraw the line
}

# Cancel any pending debounce timer
_waz_cancel_pending() {
    if (( _WAZ_DEBOUNCE_PID > 0 )); then
        kill $_WAZ_DEBOUNCE_PID 2>/dev/null
        _WAZ_DEBOUNCE_PID=0
    fi
}

# Schedule a prediction after 500ms debounce (runs in background, never blocks)
_waz_schedule_predict() {
    local buf="$BUFFER"
    local cwd="$PWD"
    local sid="$WAZ_SESSION_ID"
    local fd=$_WAZ_ASYNC_FD
    local is_proactive="$1"

    {
        # Debounce: wait 500ms (skip delay for proactive on empty prompt)
        [[ "$is_proactive" != "1" ]] && sleep 0.5

        local pred
        if [[ -n "${buf// /}" ]]; then
            pred=$(command waz predict --cwd "$cwd" --session "$sid" --prefix "$buf" --fast 2>/dev/null)
        else
            pred=$(command waz predict --cwd "$cwd" --session "$sid" 2>/dev/null)
        fi

        # Write result to FIFO → triggers zle -F callback
        [[ -n "$pred" ]] && print -u $fd "$pred"
    } &!
    _WAZ_DEBOUNCE_PID=$!
}

# --- Clear ghost text ---
_waz_clear() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""
    region_highlight=("${(@)region_highlight:#*fg=#6c6c6c*}")
}

# --- Widget: proactive suggestion when new prompt appears ---
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
    [[ -n "$_WAZ_ASYNC_FD" ]] && exec {_WAZ_ASYNC_FD}>&- 2>/dev/null
}
add-zsh-hook zshexit _waz_cleanup

# --- Initialize async FIFO ---
_waz_setup_async

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

    # Check if this looks like natural language
    if command waz check-nl -- $full_input 2>/dev/null; then
        # It's natural language — ask the AI
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
