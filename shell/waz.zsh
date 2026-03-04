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
        # 2+ chars typed: fast local prediction (no LLM)
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
