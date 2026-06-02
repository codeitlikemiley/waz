use super::{PASSWORD_PROMPT_REGEX, SU_ROOT_CMD_REGEX, is_su_to_root};

fn pw_matches(input: &str) -> bool {
    PASSWORD_PROMPT_REGEX.is_match(input.as_bytes())
}

fn su_matches(input: &str) -> bool {
    SU_ROOT_CMD_REGEX.is_match(input.as_bytes())
}

#[test]
fn password_prompt_matches_typical_forms() {
    // half-width colon
    assert!(pw_matches("Password:"));
    assert!(pw_matches("Password: "));
    assert!(pw_matches("[sudo] password for alice: "));
    assert!(pw_matches("user@host's password: "));
    // Full-width colon (Chinese input method)
    assert!(pw_matches("密码:"));
    assert!(pw_matches("密码："));
    // Galaxy Kirin V10 No Colon Special Case
    assert!(pw_matches("输入密码"));
    assert!(pw_matches("输入密码 "));
    // passphrase
    assert!(pw_matches("Enter passphrase for key '/home/u/.ssh/id_rsa': "));
}

#[test]
fn password_prompt_rejects_false_positives() {
    // These are outputs that contain 'password' / 'password' but are not real prompts, and cannot be false positives.
    assert!(!pw_matches("Your password has expired"));
    assert!(!pw_matches("Bad password, try again"));
    assert!(!pw_matches("password changed successfully"));
    assert!(!pw_matches("New password for root"));
    assert!(!pw_matches("Welcome! Please change your password soon.\n"));
    assert!(!pw_matches("Last login: Mon Jan 1 password rotated yesterday\n"));
    // Same as Chinese
    assert!(!pw_matches("您的密码已过期"));
}

#[test]
fn su_root_matches_common_variants() {
    // The most basic
    assert!(su_matches("su"));
    assert!(su_matches("su\n"));
    // Shortcut without username (default root)
    assert!(su_matches("su -"));
    assert!(su_matches("su -l"));
    assert!(su_matches("su --login"));
    // explicit root
    assert!(su_matches("su root"));
    assert!(su_matches("su - root"));
    assert!(su_matches("su -l root"));
    assert!(su_matches("su --login root"));
    // sudo su(\bsu can still hit)
    assert!(su_matches("sudo su"));
}

#[test]
fn su_to_other_user_does_not_match() {
    // Switching to a non-root user should not trigger
    assert!(!su_matches("su lg"));
    assert!(!su_matches("su - lg"));
    assert!(!su_matches("su -l lg"));
    assert!(!su_matches("su --login lg"));
    assert!(!su_matches("su admin"));
}

#[test]
fn su_in_middle_of_other_command_does_not_match() {
    // su should not trigger if it is not at the end of the line
    assert!(!su_matches("susan"));
    assert!(!su_matches("issue"));
    // For commands like grep su file, the end of the line is neither su nor su root mode.
    assert!(!su_matches("grep su /etc/passwd"));
}

#[test]
fn is_su_to_root_detects_in_buffer() {
    let buf = b"user@host:~$ su root\r\nPassword: ";
    assert!(is_su_to_root(buf));

    let buf = b"user@host:~$ su lg\r\nPassword: ";
    assert!(!is_su_to_root(buf));
}

#[test]
fn full_pipeline_su_root_with_password_prompt() {
    // Simulate the complete PTY sequence: the user enters `su -`, and a password prompt appears after the echo
    let buf = b"alice@kylin:~$ su -\r\n\xe5\xaf\x86\xe7\xa0\x81\xef\xbc\x9a";
    assert!(PASSWORD_PROMPT_REGEX.is_match(buf));
    assert!(is_su_to_root(buf));
}
