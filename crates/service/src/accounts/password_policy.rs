pub const MINIMUM_NEW_PASSWORD_LENGTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewPasswordTooShort {
    pub minimum: usize,
}

pub fn validate_new_password(password: &str) -> Result<(), NewPasswordTooShort> {
    if password.trim().chars().count() < MINIMUM_NEW_PASSWORD_LENGTH {
        return Err(NewPasswordTooShort {
            minimum: MINIMUM_NEW_PASSWORD_LENGTH,
        });
    }
    Ok(())
}
