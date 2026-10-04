"""Exercise a running service with synthetic accounts in an empty data store."""

import concurrent.futures
import json
import sys
import time
import urllib.error
import urllib.request


def main():
    base = sys.argv[1].rstrip("/")

    def request(path, payload=None):
        data = None if payload is None else json.dumps(payload).encode()
        req = urllib.request.Request(
            base + path, data=data, headers={"Content-Type": "application/json"}
        )
        with urllib.request.urlopen(req, timeout=30) as response:
            return json.load(response)

    for attempt in range(60):
        try:
            config = request("/config")
            break
        except (urllib.error.URLError, ConnectionError, TimeoutError):
            if attempt == 59:
                raise
            time.sleep(1)
    assert config["app_version"], "missing application version"
    assert request("/accounts") == [], "verification requires an empty isolated account store"
    with urllib.request.urlopen(base + "/", timeout=10) as response:
        page = response.read().decode()
    for field in ("manual-cookie-aliases", "manual-cookie-emails", "manual-cookie-entries"):
        assert field in page, f"missing UI field: {field}"
    assert request("/runtime/updates/prepare", {})["ready"] is True
    assert request("/runtime/updates/cancel", {})["ready"] is False

    aliases = [
        "Case", "case", "a b", "a_x20_b", "a:b", "a_x3A_b", "under_score",
        "under_x5F_score", "\u4eca\u5929", "_xE4__xBB__x8A__xE5__xA4__xA9_",
        "\u00e9", "e\u0301",
    ]
    accounts = {
        alias: {
            "email": f"native-{index}@example.test",
            "password": f"fixture-password-{index}",
            "cookies": {"overleaf_session2": f"fixture-cookie-{index}"},
            "git_token": f"fixture-token-{index}",
        }
        for index, alias in enumerate(aliases)
    }

    def import_accounts(records):
        return request("/accounts/import", {
            "json": json.dumps({"accounts": records}),
            "refresh_session_metadata": False,
            "fetch_git_token": False,
        })

    assert import_accounts(accounts)["imported_count"] == len(accounts)

    def exported():
        return request("/accounts/export/json", {"aliases": ",".join(aliases)})["accounts"]

    def assert_secrets(actual, expected):
        for alias, record in expected.items():
            for field in ("email", "password", "cookies", "git_token"):
                assert actual[alias][field] == record[field], f"credential mismatch: {alias}/{field}"

    assert_secrets(exported(), accounts)
    for index, alias in enumerate(aliases):
        password = f"updated-password-{index}"
        assert request("/accounts/password", {"aliases": alias, "passwords": password})["updated_count"] == 1
        accounts[alias]["password"] = password
    assert_secrets(exported(), accounts)

    repeated = {"different-alias": dict(accounts[aliases[0]], email=" NATIVE-0@EXAMPLE.TEST ")}
    duplicate = import_accounts(repeated)
    assert duplicate["imported_count"] == 0
    assert duplicate["skipped_duplicate_email_count"] == 1
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
        results = list(executor.map(lambda _: import_accounts({"parallel": {
            "email": "parallel@example.test", "password": "parallel-password"
        }}), range(3)))
    assert sum(result["imported_count"] for result in results) == 1
    assert sum(result["skipped_duplicate_email_count"] for result in results) == 2
    assert_secrets(exported(), accounts)

    removed = request("/accounts/remove", {
        "aliases": aliases[0], "cleanup_remote_projects": False, "confirm_local_only": True,
    })
    assert removed["removed_count"] == 1
    aliases.pop(0)
    accounts.pop("Case")
    assert_secrets(exported(), accounts)
    request("/accounts/remove", {
        "aliases": ",".join(aliases + ["parallel"]),
        "cleanup_remote_projects": False, "confirm_local_only": True,
    })
    assert request("/accounts") == []
    print("PASS: startup, UI assets, native credentials, import/export, password updates, email deduplication, concurrent imports, local removal, update preparation")


if __name__ == "__main__":
    main()
