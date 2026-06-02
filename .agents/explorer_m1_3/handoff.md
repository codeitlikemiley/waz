# Handoff Report

## 1. Observation
The following code structures and behaviors were observed:
- In `/Volumes/goldcoders/zap/app/src/terminal/input.rs` (lines 9247-9297), the `EditorEvent::Edited` handler under `InputSuggestionsMode::TmpFormPanel` matches:
  ```rust
  InputSuggestionsMode::TmpFormPanel {
      command_entry,
      active_token_index,
      token_values,
  } => {
      let buffer_text = self.buffer_text(ctx);
      let entry = command_entry.clone();
      let active_idx = *active_token_index;
      ...
      let new_vals = warp_completer::signatures::tmp::extract_token_values(&entry.command, &entry.tokens, &buffer_text);
      if new_vals != *token_values {
          self.suggestions_mode_model.update(ctx, |m, ctx| {
              m.set_mode(
                  InputSuggestionsMode::TmpFormPanel {
                      command_entry: entry.clone(),
                      active_token_index: active_idx,
                      token_values: new_vals,
                  },
                  ctx,
              );
          });
      }
      ...
  ```
- In `/Volumes/goldcoders/zap/app/src/terminal/input.rs` (lines 12008-12028), the `input_enter` handler under `InputSuggestionsMode::TmpFormPanel` executes the command immediately without checking if `input_suggestions` is open or has items:
  ```rust
  if let InputSuggestionsMode::TmpFormPanel {
      command_entry,
      active_token_index: _,
      token_values,
  } = self.suggestions_mode_model.as_ref(ctx).mode().clone() {
      for (i, token) in command_entry.tokens.iter().enumerate() {
          if token.required && token_values[i].trim().is_empty() {
              self.suggestions_mode_model.update(ctx, |m, ctx| {
                  m.set_mode(
                      InputSuggestionsMode::TmpFormPanel {
                          command_entry: command_entry.clone(),
                          active_token_index: i,
                          token_values: token_values.clone(),
                      },
                      ctx,
                  );
              });
              return;
          }
      }
  
      let assembled = warp_completer::signatures::tmp::build_assembled_command(&command_entry, &token_values, false);
      ...
  ```
- In `/Volumes/goldcoders/zap/app/src/terminal/input.rs` (lines 11379-11401), the `input_shift_tab` handler under `InputSuggestionsMode::TmpFormPanel` always shifts the active token index without checking the completions menu overlay:
  ```rust
  InputSuggestionsMode::TmpFormPanel {
      command_entry,
      active_token_index,
      token_values,
  } => {
      let entry = command_entry.clone();
      let vals = token_values.clone();
      let mut idx = *active_token_index;
      if idx > 0 {
          idx -= 1;
      }
      self.suggestions_mode_model.update(ctx, |m, ctx| {
          m.set_mode(
              InputSuggestionsMode::TmpFormPanel {
                  command_entry: entry,
                  active_token_index: idx,
                  token_values: vals,
              },
              ctx,
          );
      });
      return;
  }
  ```

---

## 2. Logic Chain
1. When a user cycles or selects a suggestion in the completion menu overlay (via Arrow Keys, Tab, or clicking), a programmatic edit is performed:
   `editor.set_buffer_text_ignoring_undo(&assembled, ctx)`.
2. This emits an `EditorEvent::Edited(edit_origin)` with a non-user origin (`EditOrigin::SystemEdit`).
3. In `handle_editor_event`, under `TmpFormPanel`, this event causes `extract_token_values` to parse the new buffer text.
4. Because the assembled buffer text lacks placeholders, `extract_token_values` parses it in a lossy/imperfect way, causing `new_vals` to differ from the exact selected `token_values`.
5. This updates `self.suggestions_mode_model` with the corrupted `new_vals`, overwriting the correct selected values.
6. Restricting the re-parsing step to user-initiated edits (`edit_origin.is_user()`) will prevent programmatic selection edits from corrupting the state.
7. During text input, if the suggestions menu overlay is open (e.g. for a `File` parameter), pressing Enter should select and confirm the highlighted suggestion (`suggestions.confirm(ctx)`) rather than running the command.
8. Similarly, Shift-Tab should cycle backwards through the open completions overlay (`suggestions.select_prev(ctx)`) rather than switching active input fields.

---

## 3. Caveats
- No caveats. We have examined the complete control flows for typing, navigating, selecting, and executing under the `TmpFormPanel` mode.

---

## 4. Conclusion
The out-of-sync behavior between the `TmpFormPanel` token state and the editor buffer is caused by:
1. Programmatic edits (triggered by selecting suggestions) being processed by `handle_editor_event`'s lossy re-parser, overwriting the clean state.
2. The Enter key bypassing suggestion confirmation and executing the command.
3. The Shift-Tab key bypassing suggestion cycling and shifting focus to the previous parameter.

Applying the remediation plan detailed in `findings.md` will standardize event propagation and resolve all three synchronization issues.

---

## 5. Verification Method
- **Syntax and Type Safety Verification**: Run `cargo check` in the repository root to verify that the proposed changes are syntactically valid and compile against the GPUI views.
- **Files to Inspect**:
  - `/Volumes/goldcoders/zap/app/src/terminal/input.rs`
  - `/Volumes/goldcoders/zap/app/src/terminal/tmp_panel.rs`
  - `/Volumes/goldcoders/zap/crates/warp_completer/src/signatures/tmp.rs`
- **Invalidation Condition**: If `EditOrigin::is_user` is changed or deprecated, the check in `handle_editor_event` would need to be updated.
