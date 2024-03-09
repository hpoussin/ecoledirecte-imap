use reqwest::{
    blocking::{Client, RequestBuilder},
    header::USER_AGENT,
    Url,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use crate::auth::UserId;
use crate::auth::UserId::{Eleve, Famille};

const API_VERSION: &str = "4.43.0";

pub enum MailboxId {
    Received(u32),
    Sent,
    Draft,
    Archived,
}

fn build_request<'a>(
    client: &Client,
    verbe: &'a str,
    route: &str,
    mut qs_params: HashMap<&str, &'a str>,
    json_params: Value,
    token: &str,
) -> RequestBuilder {
    let BASE_URL = Url::parse("https://api.ecoledirecte.com/").unwrap();
    qs_params.insert("verbe", verbe);
    qs_params.insert("v", API_VERSION);
    let url = Url::parse_with_params(BASE_URL.join(route).unwrap().as_str(), qs_params).unwrap();
    client
        .post(url)
        .header(USER_AGENT, "ecoledirecte-imap")
        .header("X-Token", token)
        .body("data=".to_owned() + &json_params.to_string())
}

pub fn login(
    client: &Client,
    username: &str,
    password: &str,
) -> Result<(UserId, String), Option<String>> {
    let request = build_request(
        client,
        "",
        "/v3/login.awp",
        HashMap::new(),
        json!({
            "identifiant": username,
            "motdepasse": password,
        }),
        "",
    );
    let response: Value = request.send().unwrap().json().unwrap();

    if response["code"] == json!(200) {
        let user_id = response["data"]["accounts"][0]["id"]
            .as_u64()
            .unwrap()
            .try_into()
            .unwrap();
        let user = if response["data"]["accounts"][0]["typeCompte"] == "1" {
            Famille(user_id)
        } else {
            Eleve(user_id)
        };
        Ok((
            user,
            response["token"].as_str().unwrap().to_string(),
        ))
    } else {
        Err(response["message"].as_str().map(|s: &str| s.to_string()))
    }
}

pub fn get_folder_info(client: &Client, mailbox_id: &MailboxId, user_id: UserId, token: &str) -> Value {
    let (type_recuperation, classeur_id) = match mailbox_id {
        MailboxId::Received(id) => (if *id == 0 { "received" } else { "classeur" }, *id),
        MailboxId::Sent => ("sent", 0),
        MailboxId::Draft => ("draft", 0),
        MailboxId::Archived => ("archived", 0),
    };
    let classeur_id = classeur_id.to_string();
    let url = match user_id {
        Eleve(user_id) => format!("/v3/eleves/{user_id}/messages.awp"),
        Famille(user_id) => format!("/v3/familles/{user_id}/messages.awp"),
    };
    let request = build_request(
        client,
        "get",
        &url,
        {
            let mut qs = HashMap::<&str, &str>::new();
            qs.insert("typeRecuperation", type_recuperation);
            qs.insert("idClasseur", &classeur_id);
            qs
        },
        json!({}),
        token,
    );
    request.send().unwrap().json::<Value>().unwrap()["data"].take()
}

pub fn get_folders(client: &Client, user_id: UserId, token: &str) -> Vec<(String, u32)> {
    get_folder_info(client, &MailboxId::Received(0), user_id, token)["classeurs"]
        .as_array()
        .unwrap()
        .into_iter()
        .map(|classeur| {
            (
                classeur["libelle"].as_str().unwrap().to_string(),
                classeur["id"].as_u64().unwrap() as u32,
            )
        })
        .collect()
}
