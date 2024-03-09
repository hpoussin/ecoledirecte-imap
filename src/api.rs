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
            qs.insert("getAll", "1");
            qs
        },
        json!({}),
        token,
    );
    request.send().unwrap().json::<Value>().unwrap()["data"].take()
}

pub fn get_messages(client: &Client, id: UserId, token: &str, mailbox_id: &MailboxId) -> Vec<(u32, serde_json::Value)> {
    let category = match mailbox_id {
        MailboxId::Received(_id) => "received",
        MailboxId::Sent => "sent",
        MailboxId::Draft => "draft",
        MailboxId::Archived => "archived",
    };
    get_folder_info(client, mailbox_id, id, token)["messages"][category]
        .as_array()
        .unwrap()
        .into_iter()
        .map(|message| {
            (
                message["id"].as_u64().unwrap() as u32,
                message.clone(),
            )
        })
        .collect()
}

pub fn get_message(client: &Client, user_id: UserId, token: &str, mailbox_id: &MailboxId, message_id: u32) -> serde_json::Value {
    let message_id = message_id.to_string();
    let url = match user_id {
        Eleve(user_id) => format!("/v3/eleves/{user_id}/messages/{message_id}.awp"),
        Famille(user_id) => format!("/v3/familles/{user_id}/messages/{message_id}.awp"),
    };
    let mode = match mailbox_id {
        MailboxId::Received(_) => "destinataire",
        MailboxId::Sent => "expediteur",
        MailboxId::Draft => "expediteur",
        MailboxId::Archived => todo!(),
    };
    let request = build_request(
        client,
        "get",
        &url,
        {
            let mut qs = HashMap::<&str, &str>::new();
            qs.insert("mode", mode);
            qs
        },
        json!({}),
        token,
    );
    request.send().unwrap().json::<Value>().unwrap()["data"].take()
}

pub fn get_attachment(client: &Client, token: &str, attachment_id: u32) -> bytes::Bytes {
    let attachment_id = attachment_id.to_string();
    let url = "/v3/telechargement.awp";
    let request = build_request(
        client,
        "get",
        &url,
        {
            let mut qs = HashMap::<&str, &str>::new();
            qs.insert("fichierId", attachment_id.as_str());
            qs.insert("leTypeDeFichier", "PIECE_JOINTE");
            qs
        },
        json!({}),
        token,
    );
    request.send().unwrap().bytes().unwrap()
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

pub fn set_read_status(client: &Client, user_id: UserId, token: &str, read_status: bool, message_ids: &Vec<u32>) {
    match read_status {
        true => {
            // a) only way to mark a message as read is to request it
            // b) only messages in INBOX can be marked as unread
            let _ = message_ids
                .iter()
                .map(|message_id| get_message(client, user_id, token, &MailboxId::Received(0), *message_id));
        },
        false => {
            let url = match user_id {
                Eleve(user_id) => format!("/v3/eleves/{user_id}/messages.awp"),
                Famille(user_id) => format!("/v3/familles/{user_id}/messages.awp"),
            };
            let request = build_request(
                client,
                "put",
                &url,
                HashMap::new(),
                json!({
                    "action": "marquerCommeNonLu",
                    "ids": message_ids,
                }),
                token,
            );
            request.send().unwrap();
        }
    }
}
