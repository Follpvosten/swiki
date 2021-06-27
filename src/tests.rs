use rocket::{
    http::{ContentType, Status},
    local::blocking::{Client, LocalResponse},
};
use scraper::Selector;
use serial_test::serial;
use uuid::Uuid;

use super::rocket;
use crate::{
    articles::AddRevRequest,
    settings::AdminSettings,
    users::{LoginRequest, RegisterRequest},
    ArticleIndex, Cache, Db,
};

const PASSWORD: &str = "abc123";

fn client() -> Client {
    Client::tracked(rocket()).expect("failed to create rocket client")
}
fn block_on<F, R>(fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    rocket::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(fut)
}
fn content_type_form() -> ContentType {
    ContentType::new("application", "x-www-form-urlencoded")
}

fn post_form<'a>(
    client: &'a Client,
    uri: &'static str,
    data: impl serde::Serialize,
) -> LocalResponse<'a> {
    let request_body = serde_urlencoded::to_string(data).unwrap();
    client
        .post(uri)
        .header(content_type_form())
        .body(request_body)
        .dispatch()
}

/// Helper method that returns a captcha id and its solution from a new challenge.
/// Will panic if getting any of these fails.
fn register_challenge(client: &Client) -> (Uuid, String) {
    let register_challenge_response = client.get("/u/register").dispatch();
    // We need the html.
    let body = register_challenge_response.into_string().unwrap();
    // Parse it into a document we can use.
    let document = scraper::Html::parse_document(&body);
    // Select the element which gives us the captcha id
    let selector = Selector::parse("input[name='captcha_id']").unwrap();
    let input = document.select(&selector).next().unwrap();
    // And extract it
    let value = input.value().attr("value").unwrap();
    let captcha_id: Uuid = value.parse().unwrap();
    // Here we cheat and ask the cache for the solution
    let captcha_solution = client
        .rocket()
        .state::<Cache>()
        .unwrap()
        .get_solution(captcha_id)
        .unwrap();
    (captcha_id, captcha_solution)
}

fn register_account(client: &Client, username: &str, password: &str) {
    let (captcha_id, captcha_solution) = register_challenge(client);
    // Send off the request
    let response = post_form(
        client,
        "/u/register",
        RegisterRequest {
            username: username.into(),
            password: password.into(),
            pwd_confirm: password.into(),
            captcha_id,
            captcha_solution,
        },
    );
    // If it succeeds, we're registered
    assert_eq!(
        response.status(),
        Status::Ok,
        "Failed to register: {:?}",
        response.into_string()
    );
}

fn login(client: &Client, username: &str, password: &str) {
    // This is fairly straightforward compared to registering lol
    let response = post_form(
        client,
        "/u/login",
        LoginRequest {
            username: username.into(),
            password: password.into(),
        },
    );
    // If this request succeeds, we're logged in
    assert_eq!(
        response.status(),
        Status::Ok,
        "Failed to log in: {:?}",
        response.into_string()
    );
}

/// Login with a default username and password.
/// Useful if you don't care about the user and just need a session.
fn register_and_login(client: &Client, username: &str) {
    // Register a default account
    register_account(client, username, PASSWORD);
    // Then we log in, which should give us the appropriate cookies
    login(client, username, PASSWORD);
}
fn logout(client: &Client) {
    let response = client.get("/u/logout").dispatch();
    assert_eq!(response.status(), Status::Ok);
}

#[test]
fn launch() {
    client();
}

#[test]
#[serial]
fn redirects() {
    let client = client();
    let assert_redirect = |uri: &str, location| {
        let response = client.get(dbg!(uri)).dispatch();
        assert_eq!(
            response.status(),
            Status::SeeOther,
            "body: {:?}",
            response.into_string()
        );
        assert_eq!(response.headers().get_one("Location"), Some(location));
    };
    let assert_no_redirect = |uri: &str| {
        let response = client.get(uri).dispatch();
        assert_ne!(response.status(), Status::SeeOther);
    };
    // Always redirect / to main
    assert_redirect("/", "/Main");
    // When not logged in, don't allow any edits
    assert_redirect("/Main/edit", "/u/login");
    // And you don't allow access to settings
    assert_redirect("/settings", "/u/login");
    // Also trying to "log out" while not logged in should redirect
    assert_redirect("/u/logout", "/Main");
    // while the login/register routes should not redirect
    assert_no_redirect("/u/login");
    assert_no_redirect("/u/register");
    // Login first to check the u/login and u/register redirects
    register_and_login(&client, "redirects");
    assert_redirect("/u/login", "/Main");
    assert_redirect("/u/register", "/Main");
    // Editing an article should be possible now
    assert_no_redirect("/Main/edit");
    // As well as changing your settings
    assert_no_redirect("/settings");
    // Always redirect / to main
    assert_redirect("/", "/Main");
    // Finally, logout should not redirect now, but that only works once lol
    assert_no_redirect("/u/logout");
}

#[test]
#[serial]
fn register_login_logout() {
    let client = client();
    // There should be no cookies before logging in
    assert_eq!(client.cookies().iter().count(), 0);
    // There's one cookie, the session id, when you're logged in
    register_and_login(&client, "login logout");
    assert_eq!(client.cookies().iter().count(), 1);
    assert!(client.cookies().get("session_id").is_some());
    // After logging out, no more cookies should be present
    logout(&client);
    assert_eq!(client.cookies().iter().count(), 0);
}

#[test]
#[serial]
fn basic_article_routes() {
    let client = client();
    let assert_status = |uri: &str, status: Status| {
        let response = client.get(uri).dispatch();
        assert_eq!(response.status(), status, "{}", uri);
    };
    let ok = Status::Ok;
    let notfound = Status::NotFound;
    // At the start, the Main page doesn't exist, but it's a special case
    assert_status("/Main", ok);
    // You cannot look at its revisions though, as there are none.
    assert_status("/Main/revs", notfound);
    assert_status("/Main/rev/1", notfound);
    // Search should always succeed
    assert_status("/search?q=blah", ok);
    // Same for the "create article" helper
    assert_status("/create", ok);
    // An unknown article should return 404
    assert_status("/Blahblub", notfound);
    // Same for unknown revs
    assert_status("/Main/revs/1", notfound);
    // And a combination of those
    assert_status("/Blahblub/revs/1", notfound);
    // Login so we can see the edit page
    register_and_login(&client, "basic article routes");
    assert_status("/Main/edit", ok);
}

#[test]
#[serial]
fn creating_and_editing_articles() {
    let client = client();
    // We need to be logged in for this
    register_and_login(&client, "creating and editing");

    // Let's keep a reference to the db around, it will help
    let db = client.rocket().state::<Db>().unwrap();

    // Create an actual new article
    let response = post_form(
        &client,
        "/MyNewArticle/edit",
        AddRevRequest {
            title: None,
            content: "Some content blah blah blah".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
    // We will want its id to check for the changes
    let article_id = block_on(db.article_id_by_name("MyNewArticle"))
        .unwrap()
        .expect("Inserted article's id not found");
    // Change its name (just removing the My)
    let response = post_form(
        &client,
        "/MyNewArticle/edit",
        AddRevRequest {
            title: Some("ANewArticle".into()),
            content: "Some content blah blah blah".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);

    // Verify that the old name is 404 and the new one is 200
    let response = client.get("/MyNewArticle").dispatch();
    assert_eq!(response.status(), Status::NotFound);
    let response = client.get("/ANewArticle").dispatch();
    assert_eq!(response.status(), Status::Ok);

    // Verify a reverse-lookup of the new name also works
    assert_eq!(
        block_on(db.article_id_by_name("ANewArticle")).unwrap(),
        Some(article_id)
    );
    // While we're at it, make sure the content is right
    assert_eq!(
        block_on(db.get_current_rev("ANewArticle"))
            .unwrap()
            .map(|r| r.content)
            .as_deref(),
        Some("Some content blah blah blah")
    );

    // Change the content
    let response = post_form(
        &client,
        "/ANewArticle/edit",
        AddRevRequest {
            title: Some("ANewArticle".into()),
            content: "Some *new*, **shiney** content! blah blah blah!".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
    // Verify the new content
    assert_eq!(
        block_on(db.get_current_rev("ANewArticle"))
            .unwrap()
            .map(|r| r.content)
            .as_deref(),
        Some("Some *new*, **shiney** content! blah blah blah!")
    );
    // Change both
    let response = post_form(
        &client,
        "/ANewArticle/edit",
        AddRevRequest {
            title: Some("New_Article".into()),
            content: "The same old content again blah blah blah".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);

    // This dance again
    let response = client.get("/ANewArticle").dispatch();
    assert_eq!(response.status(), Status::NotFound);
    let response = client.get("/New_Article").dispatch();
    assert_eq!(response.status(), Status::Ok);

    // Verify both
    assert_eq!(
        block_on(db.article_id_by_name("New_Article")).unwrap(),
        Some(article_id)
    );
    // While we're at it, make sure the content is right
    assert_eq!(
        block_on(db.get_current_rev("New_Article"))
            .unwrap()
            .map(|r| r.content)
            .as_deref(),
        Some("The same old content again blah blah blah")
    );

    // Finally, we should also get a 200 if we submit with no changes.
    let response = post_form(
        &client,
        "/New_Article/edit",
        AddRevRequest {
            title: Some("New_Article".into()),
            content: "The same old content again blah blah blah".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
}

#[test]
#[serial]
fn search() {
    let client = client();
    // Helper to reload the search index
    let reload = || {
        client
            .rocket()
            .state::<ArticleIndex>()
            .unwrap()
            .reader
            .reload()
            .unwrap();
    };
    register_and_login(&client, "search");
    // To get some value to compare to, we just note down the length of the search page
    let first_body_length = client
        .get("/search?q=Baguette")
        .dispatch()
        .into_bytes()
        .unwrap()
        .len();
    // Add some new articles with more baguettes
    let response = post_form(
        &client,
        "/CheeseArticleOne/edit",
        AddRevRequest {
            title: None,
            content: "Some content blah blah blah Baguette".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
    let response = post_form(
        &client,
        "/NewArticle/edit",
        AddRevRequest {
            title: None,
            content: "Baguette some content blah blah blah blub".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
    let response = post_form(
        &client,
        "/Baguette/edit",
        AddRevRequest {
            title: None,
            content: "Some content blah blah blah".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
    // Force-refresh search index
    reload();
    let second_body_length = client
        .get("/search?q=Baguette")
        .dispatch()
        .into_bytes()
        .unwrap()
        .len();
    assert_ne!(first_body_length, second_body_length);
    assert!(second_body_length > first_body_length);
    // Edit an article so it doesn't contain Baguette anymore
    let response = post_form(
        &client,
        "/NewArticle/edit",
        AddRevRequest {
            title: None,
            content: "Some lame content blah blah blub".into(),
        },
    );
    assert_eq!(response.status(), Status::Ok);
    // Force-refresh search index
    reload();
    // And compare again
    let third_body_length = client
        .get("/search?q=Baguette")
        .dispatch()
        .into_bytes()
        .unwrap()
        .len();

    assert!(third_body_length < second_body_length);
    assert!(third_body_length > first_body_length);
}

#[test]
#[serial]
fn failed_register() {
    let client = client();
    // We'll test all of the ways registering can fail, oh boy
    // Helper function so we can check the output
    // This will also assert that the status is BadRequest
    let get_html = |request: &RegisterRequest| {
        let response = post_form(&client, "/u/register", request);
        assert_eq!(
            response.status(),
            Status::BadRequest,
            "request: {:?}\nresponse: {:?}",
            request,
            response.into_string()
        );
        let text = response.into_string().unwrap();
        scraper::Html::parse_document(&text)
    };
    // Helper function to check if any of the p.help.is-danger elements on the
    // given Html has the given text as content
    let assert_help_text = |html: &scraper::Html, content: &str| {
        let selector = Selector::parse("p.help.is-danger").unwrap();
        let mut elements = html.select(&selector);
        assert!(
            elements.any(|elem| elem.inner_html() == content),
            "Failed to assert help text {} (html: {})",
            content,
            html.root_element().inner_html()
        );
    };

    // No username
    let (captcha_id, captcha_solution) = register_challenge(&client);
    let request = RegisterRequest {
        username: "".into(),
        password: "password123".into(),
        pwd_confirm: "password123".into(),
        captcha_id,
        captcha_solution,
    };
    let html = get_html(&request);
    assert_help_text(&html, "You need a username!");

    // No password
    let (captcha_id, captcha_solution) = register_challenge(&client);
    let request = RegisterRequest {
        username: "Someone".into(),
        password: "".into(),
        pwd_confirm: "".into(),
        captcha_id,
        captcha_solution,
    };
    let html = get_html(&request);
    assert_help_text(&html, "The given passwords were empty or did not match!");

    // Non-matching passwords
    let (captcha_id, captcha_solution) = register_challenge(&client);
    let request = RegisterRequest {
        password: "password123".into(),
        pwd_confirm: "PassWord123".into(),
        captcha_id,
        captcha_solution,
        ..request
    };
    let html = get_html(&request);
    assert_help_text(&html, "The given passwords were empty or did not match!");

    // Invalid usernames
    let (captcha_id, captcha_solution) = register_challenge(&client);
    let mut request = RegisterRequest {
        username: "register".into(),
        password: "password123".into(),
        pwd_confirm: "password123".into(),
        captcha_id,
        captcha_solution,
    };
    let html = get_html(&request);
    assert_help_text(&html, "This username is invalid or already taken!");
    let (captcha_id, captcha_solution) = register_challenge(&client);
    request.username = "login".into();
    request.captcha_id = captcha_id;
    request.captcha_solution = captcha_solution;
    let html = get_html(&request);
    assert_help_text(&html, "This username is invalid or already taken!");

    // For an already taken username, we need to register one successfully
    register_account(&client, "Someone", "password123");
    let (captcha_id, captcha_solution) = register_challenge(&client);
    let request = RegisterRequest {
        username: "Someone".into(),
        password: "password123".into(),
        pwd_confirm: "password123".into(),
        captcha_id,
        captcha_solution,
    };
    let html = get_html(&request);
    assert_help_text(&html, "This username is invalid or already taken!");

    // Wrong captcha solution
    let (captcha_id, _solution) = register_challenge(&client);
    let request = RegisterRequest {
        username: "Someone".into(),
        password: "password123".into(),
        pwd_confirm: "password123".into(),
        captcha_id,
        // This is a definitly invalid captcha
        captcha_solution: "aAaAaA".into(),
    };
    let html = get_html(&request);
    assert_help_text(&html, "Error, please try again!");
    // Completely bollocks captcha
    let request = RegisterRequest {
        username: "Someone".into(),
        password: "password123".into(),
        pwd_confirm: "password123".into(),
        //          v ok Rocket, wtf
        captcha_id: uuid::Uuid::new_v4().to_string().parse().unwrap(),
        captcha_solution: "WXZTMWEMOUTRIXWFaaaaAAaaAAAAhaudhwkjsd".into(),
    };
    let html = get_html(&request);
    assert_help_text(&html, "Error, please try again!");
}

#[test]
#[serial]
fn admin_permissions_and_settings() {
    let client = client();
    let db = client.rocket().state::<Db>().unwrap();
    async fn load_admin(db: &Db) -> Option<String> {
        sqlx::query_scalar!(r#"SELECT name FROM "user" WHERE is_admin = TRUE"#)
            .fetch_optional(&db.pool)
            .await
            .unwrap()
    }
    let admin = match block_on(load_admin(db)) {
        Some(name) => name,
        None => {
            register_account(&client, "Admin", PASSWORD);
            "Admin".into()
        }
    };
    // Only the first account should be an admin
    register_account(&client, "User", PASSWORD);
    // Now we check if the admin flag actually gets applied
    // Log in as admin and change settings
    let admin_form_selector = Selector::parse("form[action='/settings/admin']").unwrap();
    login(&client, &admin, PASSWORD);
    let client_page = client.get("/settings").dispatch().into_string().unwrap();
    let document = scraper::Html::parse_document(&client_page);
    // Verify that we have the admin form
    assert!(document.select(&admin_form_selector).next().is_some());
    // Now send a change to the settings (registration is on by default)
    let response = post_form(
        &client,
        "/settings/admin",
        AdminSettings {
            registration_enabled: false,
        },
    );
    assert_eq!(response.status(), Status::Ok);
    // Send the same request again, just to be sure
    let response = post_form(
        &client,
        "/settings/admin",
        AdminSettings {
            registration_enabled: false,
        },
    );
    assert_eq!(response.status(), Status::Ok);
    // Now we logout
    logout(&client);
    // Verify that the register page now redirects to /Main
    let response = client.get("/u/register").dispatch();
    assert_eq!(response.status(), Status::SeeOther);
    assert_eq!(response.headers().get_one("Location"), Some("/Main"));

    // Now we log in as the non-admin user
    login(&client, "User", PASSWORD);
    // Verify that the admin section is not there
    let client_page = client.get("/settings").dispatch().into_string().unwrap();
    let document = scraper::Html::parse_document(&client_page);
    assert!(document.select(&admin_form_selector).next().is_none());
    // Trying to change the admin settings as a normal user should fail and redirect
    // TODO: Maybe this should return a good 403 error page instead?
    let response = post_form(
        &client,
        "/settings/admin",
        AdminSettings {
            registration_enabled: false,
        },
    );
    assert_eq!(response.status(), Status::SeeOther);
    assert_eq!(response.headers().get_one("Location"), Some("/settings"));

    logout(&client);
    // Reset the admin flag back to normal, just to be sure
    login(&client, &admin, PASSWORD);
    let response = post_form(
        &client,
        "/settings/admin",
        AdminSettings {
            registration_enabled: true,
        },
    );
    assert_eq!(response.status(), Status::Ok);
}
