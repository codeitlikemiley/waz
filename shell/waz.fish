# Waz — Command prediction for Fish
# Add to your ~/.config/fish/config.fish: waz init fish | source

# ── Session setup ───────────────────────────────────────────────────
if not set -q WAZ_SESSION_ID
    set -gx WAZ_SESSION_ID (waz session-id)
end

# ── Record commands after execution ─────────────────────────────────
function _waz_postexec --on-event fish_postexec
    set -l cmd $argv[1]
    set -l exit_code $status
    if test -n "$cmd"
        command waz record --cwd "$PWD" --session "$WAZ_SESSION_ID" --exit-code "$exit_code" -- "$cmd" &>/dev/null &
    end
end

# ── Custom keybinding for predictions ───────────────────────────────
function _waz_predict
    set -l buf (commandline --current-buffer)
    if test -n "$buf"
        set -l prediction (command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" --prefix "$buf" 2>/dev/null)
        if test -n "$prediction"
            commandline --replace "$prediction"
            commandline --cursor (string length "$prediction")
        end
    end
end

# Bind Ctrl+Space to prediction
bind \c@ '_waz_predict'
