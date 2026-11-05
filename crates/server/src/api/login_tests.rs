//! Login rate limiting and lockout through the API (PRD §9.2).
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use crate::store::audit::Filter;
use crate::store::users::{LoginLimits, Role};
use crate::test_support::{Client, PASSWORD, anonymous, state_with};

fn limits(max_failures: u32, client_attempts_per_minute: u32) -> crate::Options {
    crate::Options {
        login: LoginLimits {
            max_failures,
            lockout: Duration::from_secs(15 * 60),
            // High, so these tests see lockout without waiting between tries.
            slow_after: 100,
            client_attempts_per_minute,
        },
        ..crate::Options::default()
    }
}

async fn try_login(
    client: &Client,
    username: &str,
    password: &str,
) -> (StatusCode, serde_json::Value) {
    client
        .post(
            "/api/login",
            json!({ "username": username, "password": password }),
        )
        .await
}

async fn actions(client: &Client) -> Vec<String> {
    client
        .state
        .store()
        .audit_entries(&Filter::default())
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect()
}

#[tokio::test]
async fn a_locked_account_is_refused_even_with_the_right_password_until_unlocked() {
    let state = state_with(limits(3, 100)).await;
    let admin = Client::with_state(state.clone()).await;
    state
        .store()
        .create_user("sam", PASSWORD, Role::Operator)
        .await
        .unwrap();
    let visitor = anonymous(&state);

    for _ in 0..2 {
        assert_eq!(
            try_login(&visitor, "sam", "wrong one").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    // The third failure locks the account.
    assert_eq!(
        try_login(&visitor, "sam", "wrong one").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = try_login(&visitor, "sam", PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body["error"].as_str().unwrap().contains("locked"), "{body}");
    assert!(body["retry_after_secs"].as_u64().unwrap() > 800);

    // The lockout is in the audit log, by Gantry, naming the account.
    let locked = state
        .store()
        .audit_entries(&Filter {
            action: Some("account.locked".into()),
            ..Filter::default()
        })
        .await
        .unwrap();
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].username, None);
    assert_eq!(locked[0].target.as_deref(), Some("sam"));

    // Admins see the lock, and can end it.
    let (_, users) = admin.get("/api/users").await;
    let sam = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "sam")
        .unwrap()
        .clone();
    assert!(sam["locked_until"].is_string());
    let (status, body) = admin
        .post(&format!("/api/users/{}/unlock", sam["id"]), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["locked_until"].is_null());
    assert_eq!(actions(&admin).await[0], "account.unlocked");

    assert_eq!(try_login(&visitor, "sam", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn only_admins_unlock_accounts() {
    let state = state_with(limits(3, 100)).await;
    let admin = Client::with_state(state.clone()).await;
    let operator = admin.as_role(Role::Operator).await;
    let (_, me) = operator.get("/api/session").await;
    let (status, _) = operator
        .post(&format!("/api/users/{}/unlock", me["id"]), json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn one_client_is_slowed_whichever_accounts_it_tries() {
    let state = state_with(limits(100, 4)).await;
    let visitor = anonymous(&state);
    for name in ["a", "b", "c", "d"] {
        assert_eq!(
            try_login(&visitor, name, "guess").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let response = visitor
        .post(
            "/api/login",
            json!({ "username": "e", "password": "guess" }),
        )
        .await;
    assert_eq!(response.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.1["error"].as_str().unwrap().contains("from here"));
}

#[tokio::test]
async fn repeated_failures_are_slowed_before_the_lock() {
    let state = state_with(crate::Options {
        login: LoginLimits {
            max_failures: 10,
            slow_after: 2,
            ..LoginLimits::default()
        },
        ..crate::Options::default()
    })
    .await;
    state
        .store()
        .create_user("sam", PASSWORD, Role::Viewer)
        .await
        .unwrap();
    let visitor = anonymous(&state);
    for _ in 0..2 {
        assert_eq!(
            try_login(&visitor, "sam", "nope").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    // Straight away again: told to wait, even with the right password.
    let (status, body) = try_login(&visitor, "sam", PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        body["error"].as_str().unwrap().contains("Wait 1 s"),
        "{body}"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(try_login(&visitor, "sam", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn successful_logins_never_count_against_a_client() {
    let state = state_with(limits(100, 2)).await;
    state
        .store()
        .create_user("sam", PASSWORD, Role::Viewer)
        .await
        .unwrap();
    let visitor = anonymous(&state);
    for _ in 0..10 {
        assert_eq!(try_login(&visitor, "sam", PASSWORD).await.0, StatusCode::OK);
    }
    // Failures do.
    for _ in 0..2 {
        assert_eq!(
            try_login(&visitor, "sam", "nope").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        try_login(&visitor, "sam", PASSWORD).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_burst_of_concurrent_guesses_is_held_to_the_lockout() {
    let state = state_with(limits(3, 1000)).await;
    state
        .store()
        .create_user("sam", PASSWORD, Role::Viewer)
        .await
        .unwrap();
    let visitor = anonymous(&state);
    let guesses = (0..20).map(|i| {
        let visitor = &visitor;
        async move { try_login(visitor, "sam", &format!("guess {i}")).await.0 }
    });
    let statuses = futures::future::join_all(guesses).await;
    let checked = statuses
        .iter()
        .filter(|s| **s == StatusCode::UNAUTHORIZED)
        .count();
    assert_eq!(
        checked, 3,
        "only three guesses are ever checked: {statuses:?}"
    );
    assert_eq!(
        try_login(&visitor, "sam", PASSWORD).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_name_that_isnt_an_account_is_refused_just_like_one_that_is() {
    let state = state_with(limits(3, 1000)).await;
    state
        .store()
        .create_user("sam", PASSWORD, Role::Viewer)
        .await
        .unwrap();
    let visitor = anonymous(&state);
    let mut replies = Vec::new();
    for name in ["sam", "nobody"] {
        let mut seen = Vec::new();
        for _ in 0..5 {
            let (status, body) = try_login(&visitor, name, "wrong guess").await;
            seen.push((status, body["error"].clone()));
        }
        replies.push(seen);
    }
    assert_eq!(replies[0], replies[1]);
    assert_eq!(replies[0][3].0, StatusCode::TOO_MANY_REQUESTS);
}
