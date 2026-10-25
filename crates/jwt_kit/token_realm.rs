use std::ops::Deref;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenRealm {
    Account,
}

impl Deref for TokenRealm {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            TokenRealm::Account => "account",
        }
    }
}
