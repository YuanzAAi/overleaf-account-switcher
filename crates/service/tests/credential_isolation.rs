use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use overleaf_api::session::OverleafUserInfo;
use overleaf_service::account_secrets::*;
use overleaf_service::{
    apply_account_import_with_backend, manual_cookie_import_candidate,
    validate_manual_cookie_import_candidates, AccountSessionError, AccountSessionIdentityValidator,
};
use overleaf_storage::{
    account_password_secret_key, AccountRecord, AccountStore, AccountsDocument, SecretBackend,
    SecretBackendError, SecretReference, SystemKeyringSecretBackend,
};

// 每次核验使用独立服务名，退出时删除本次写入的原生凭据。
struct NativeSecrets {
    backend: SystemKeyringSecretBackend,
    references: Mutex<Vec<SecretReference>>,
}

impl NativeSecrets {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self {
            backend: SystemKeyringSecretBackend::new(format!(
                "overleaf-check-{}-{nonce}",
                std::process::id()
            )),
            references: Mutex::new(Vec::new()),
        }
    }
}

impl SecretBackend for NativeSecrets {
    fn backend_id(&self) -> &str {
        self.backend.backend_id()
    }

    fn write_secret(
        &self,
        reference: &SecretReference,
        value: &str,
    ) -> Result<(), SecretBackendError> {
        let mut references = self.references.lock().unwrap();
        if !references.contains(reference) {
            references.push(reference.clone());
        }
        drop(references);
        self.backend.write_secret(reference, value)
    }

    fn read_secret(&self, reference: &SecretReference) -> Result<String, SecretBackendError> {
        self.backend.read_secret(reference)
    }

    fn delete_secret(&self, reference: &SecretReference) -> Result<(), SecretBackendError> {
        self.backend.delete_secret(reference)
    }
}

impl Drop for NativeSecrets {
    fn drop(&mut self) {
        for reference in self.references.get_mut().unwrap() {
            self.backend
                .delete_secret(reference)
                .expect("native credential cleanup failed");
        }
    }
}

#[test]
#[ignore = "requires an unlocked native credential store"]
fn native_account_credential_isolation() {
    let backend = NativeSecrets::new();
    let aliases = [
        "Arthur",
        "arthur",
        "a b",
        "a_x20_b",
        "a:b",
        "a_x3A_b",
        "under_score",
        "under_x5F_score",
        "\u{4eca}\u{5929}",
        "_xE4__xBB__x8A__xE5__xA4__xA9_",
        "\u{e9}",
        "e\u{301}",
        "unknown",
    ];
    let mut records = Vec::new();
    let mut keys = BTreeSet::new();
    for (index, alias) in aliases.iter().enumerate() {
        let mut record = AccountRecord::default();
        write_account_password_secret(&mut record, alias, &format!("password-{index}"), &backend)
            .unwrap();
        write_account_git_token_secret(&mut record, alias, &format!("token-{index}"), &backend)
            .unwrap();
        write_account_cookie_secrets(
            &mut record,
            alias,
            &BTreeMap::from([
                ("overleaf_session2".into(), format!("cookie-{index}")),
                ("x y".into(), format!("space-{index}")),
                ("x_x20_y".into(), format!("literal-{index}")),
            ]),
            &backend,
        )
        .unwrap();
        for reference in record
            .password_ref
            .iter()
            .chain(record.git_token_ref.iter())
            .chain(record.cookie_refs.values())
        {
            assert!(
                keys.insert(reference.key.to_ascii_lowercase()),
                "credential key collision"
            );
        }
        records.push(record);
    }
    let directory = tempfile::tempdir().unwrap();
    let store = AccountStore::new(directory.path().join("accounts.json"));
    store
        .save(&AccountsDocument {
            accounts: aliases
                .iter()
                .zip(&records)
                .map(|(alias, record)| (alias.to_string(), record.clone()))
                .collect(),
            ..Default::default()
        })
        .unwrap();
    let loaded = store.load().unwrap();
    std::thread::scope(|scope| {
        for (index, alias) in aliases.iter().enumerate() {
            let backend = &backend;
            let mut record = loaded.accounts[*alias].clone();
            scope.spawn(move || {
                assert_eq!(
                    read_account_password_secret(&record, alias, backend).unwrap(),
                    format!("password-{index}")
                );
                assert_eq!(
                    read_account_git_token_secret(&record, alias, backend).unwrap(),
                    format!("token-{index}")
                );
                let cookies = resolve_account_cookies(&record, alias, backend).unwrap();
                assert_eq!(cookies["overleaf_session2"], format!("cookie-{index}"));
                assert_eq!(cookies["x y"], format!("space-{index}"));
                assert_eq!(cookies["x_x20_y"], format!("literal-{index}"));
                write_account_password_secret(
                    &mut record,
                    alias,
                    &format!("updated-{index}"),
                    backend,
                )
                .unwrap();
            });
        }
    });
    for (index, alias) in aliases.iter().enumerate() {
        assert_eq!(
            read_account_password_secret(&records[index], alias, &backend).unwrap(),
            format!("updated-{index}")
        );
    }

    let mut misbound = records[1].clone();
    assert!(read_account_password_secret(&misbound, aliases[0], &backend).is_err());
    write_account_password_secret(&mut misbound, aliases[0], "repaired-password", &backend)
        .unwrap();
    assert_eq!(
        read_account_password_secret(&records[1], aliases[1], &backend).unwrap(),
        "updated-1"
    );

    for alias in ["LegacyCase", "legacycase"] {
        let reference = SecretReference::new("password", format!("account:{alias}:password"));
        backend.write_secret(&reference, alias).unwrap();
    }
    for alias in ["LegacyCase", "legacycase"] {
        let mut record = AccountRecord {
            password_ref: Some(SecretReference::new(
                "password",
                format!("account:{alias}:password"),
            )),
            ..Default::default()
        };
        assert_eq!(
            read_account_password_secret(&record, alias, &backend).unwrap(),
            alias
        );
        write_account_password_secret(&mut record, alias, "new-password", &backend).unwrap();
        assert_eq!(
            record.password_ref.as_ref().unwrap().key,
            account_password_secret_key(alias)
        );
        assert_eq!(
            read_account_password_secret(&record, alias, &backend).unwrap(),
            "new-password"
        );
    }
    let ambiguous = SecretReference::new("password", "account:a_x20_b:password");
    backend
        .write_secret(&ambiguous, "unattributable-old-value")
        .unwrap();
    for alias in ["a b", "a_x20_b"] {
        let mut record = AccountRecord {
            password_ref: Some(ambiguous.clone()),
            ..Default::default()
        };
        assert!(matches!(
            read_account_password_secret(&record, alias, &backend),
            Err(AccountSecretStoreError::Missing { .. })
        ));
        write_account_password_secret(&mut record, alias, alias, &backend).unwrap();
        assert_eq!(
            read_account_password_secret(&record, alias, &backend).unwrap(),
            alias
        );
    }
    for (index, record) in records.iter().enumerate() {
        if index % 2 == 0 {
            for reference in record
                .password_ref
                .iter()
                .chain(record.git_token_ref.iter())
                .chain(record.cookie_refs.values())
            {
                backend.delete_secret(reference).unwrap();
                assert!(matches!(
                    backend.read_secret(reference),
                    Err(SecretBackendError::SecretNotFound { .. })
                ));
            }
        } else {
            assert_eq!(
                read_account_git_token_secret(record, aliases[index], &backend).unwrap(),
                format!("token-{index}")
            );
        }
    }
    println!("native credential isolation: case, escapes, Unicode, concurrent writes, legacy ownership, deletion passed");
}

struct CookieIdentity;

#[async_trait::async_trait]
impl AccountSessionIdentityValidator for CookieIdentity {
    async fn validate(
        &self,
        _alias: &str,
        expected_email: Option<&str>,
        cookies: &BTreeMap<String, String>,
    ) -> Result<OverleafUserInfo, AccountSessionError> {
        assert!(expected_email.is_none());
        Ok(OverleafUserInfo {
            email: Some(format!("{}@example.test", cookies["overleaf_session2"])),
            user_id: None,
        })
    }
}

#[test]
#[ignore = "requires an unlocked native credential store"]
fn native_cookie_identity_email_deduplication() {
    let backend = NativeSecrets::new();
    let mut document = AccountsDocument::default();
    document.accounts.insert(
        "saved-alias".into(),
        AccountRecord {
            email: Some(" Existing@Example.Test ".into()),
            ..Default::default()
        },
    );
    let mut candidates = vec![
        manual_cookie_import_candidate(None, "", "overleaf_session2=existing"),
        manual_cookie_import_candidate(None, "", "overleaf_session2=fresh"),
        manual_cookie_import_candidate(Some("another-alias".into()), "", "overleaf_session2=fresh"),
    ];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let validation = runtime.block_on(validate_manual_cookie_import_candidates(
        &candidates,
        &CookieIdentity,
    ));
    assert_eq!(validation.valid_count, 3);
    for item in validation.items {
        candidates[item.index].record.email = Some(item.email);
    }
    let report = apply_account_import_with_backend(&mut document, &candidates, &backend).unwrap();
    assert_eq!(report.imported_count, 1);
    assert_eq!(report.skipped_duplicate_email_count, 2);
    assert_eq!(document.accounts.len(), 2);
    assert_eq!(
        resolve_account_cookies(&document.accounts["fresh"], "fresh", &backend).unwrap()
            ["overleaf_session2"],
        "fresh"
    );
    let again = apply_account_import_with_backend(&mut document, &candidates, &backend).unwrap();
    assert_eq!(again.imported_count, 0);
    assert_eq!(again.skipped_duplicate_email_count, 3);
    println!("Cookie-derived email deduplication: saved accounts, case, whitespace, batch duplicates and repeated import passed");
}
