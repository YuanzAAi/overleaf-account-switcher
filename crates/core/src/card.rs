#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardStatus {
    New,
    Used,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub cvc: String,
    pub status: CardStatus,
    pub last_error: Option<String>,
}

pub fn looks_like_payment_card_number(token: &str) -> bool {
    let digits: String = token
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    luhn_is_valid(&digits)
}

fn luhn_is_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for digit in digits.chars().rev() {
        let Some(mut value) = digit.to_digit(10) else {
            return false;
        };
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum > 0 && sum.is_multiple_of(10)
}

impl Card {
    pub fn last_four(&self) -> &str {
        let len = self.number.len();
        if len <= 4 {
            &self.number
        } else {
            &self.number[len - 4..]
        }
    }
}
