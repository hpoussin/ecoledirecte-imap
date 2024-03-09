use imap_codec::imap_types::{
    core::{Literal, NString, Tag},
    fetch::{MessageDataItem, MacroOrMessageDataItemNames, MessageDataItemName, MacroOrMessageDataItemNames::{Macro, MessageDataItemNames}},
    flag::{Flag, FlagFetch},
    sequence::{Sequence, SequenceSet, SeqOrUid},
    response::{Data, Response, Status},
};
use std::num::NonZeroU32;
use chrono::NaiveDateTime;
use mime_sniffer::MimeTypeSniffer;
use crate::NonEmptyVec;

fn make_person(person: &serde_json::Value) -> String {
    format!("\"{}\" <>", person["name"].as_str().unwrap())
}

fn make_header(message: &serde_json::Value) -> String {
    let date = NaiveDateTime::parse_from_str(message["date"].as_str().unwrap(), "%Y-%m-%d %H:%M:%S").unwrap();
    let date = date.and_local_timezone(chrono::Local).unwrap();

    let response_id = message["responseId"].as_u64().unwrap();
    let forward_id = message["forwardId"].as_u64().unwrap();

    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut cci = Vec::new();
    for person in message["to"].as_array().unwrap() {
        (match person["to_cc_cci"].as_str().unwrap() {
            "to" => &mut to,
            "cc" => &mut cc,
            "cci" => &mut cci,
            _ => panic!("to_cc_cci = {} not supported", person["to_cc_cci"].as_str().unwrap())
        }).push(make_person(person));
    }
    if message["mtype"].as_str().unwrap() == "received" {
        to.push("Me".to_string())
    }

    let mut headers = vec![
        format!("Subject: {}", message["subject"].as_str().unwrap()),
        format!("Date: {}", date.to_rfc2822()),
        "MIME-Version: 1.0".to_string(),
        format!("From: {}", make_person(&message["from"])),
        format!("Message-ID: <{}@>", message["id"].as_u64().unwrap())
    ];
    if to.len() > 0 { headers.push(format!("To: {}", to.join(",\r\n "))) }
    if cc.len() > 0 { headers.push(format!("Cc: {}", cc.join(",\r\n "))) }
    if cci.len() > 0 { headers.push(format!("Cci: {}", cci.join(",\r\n "))) }
    if response_id > 0 { headers.push(format!("In-Reply-To: <{}@>", response_id)) }
    if forward_id > 0 { headers.push(format!("Recent-Message-ID: <{}@>", forward_id)) }

    headers.join("\r\n")
}

fn get_item<'a, F: Fn(u32) -> serde_json::Value, G: Fn(u32) -> bytes::Bytes>(item: &MessageDataItemName, message: &serde_json::Value, get_message: F, get_attachment: G) -> Option<MessageDataItem<'a>> {
    match item {
        MessageDataItemName::Flags => {
            let mut flags = Vec::new();
            if message["read"].as_bool().unwrap() {
                flags.push(FlagFetch::Flag(Flag::Seen));
            }
            if message["answered"].as_bool().unwrap() {
                flags.push(FlagFetch::Flag(Flag::Answered));
            }
            if message["brouillon"].as_bool().unwrap() {
                flags.push(FlagFetch::Flag(Flag::Draft));
            }
            Some(MessageDataItem::Flags(flags))
        },
        MessageDataItemName::Uid =>
            Some(MessageDataItem::Uid(NonZeroU32::new(message["id"].as_u64().unwrap() as u32).unwrap())),
        MessageDataItemName::Rfc822Size => None,
        MessageDataItemName::Rfc822Header =>
            Some(MessageDataItem::Rfc822Header(Literal::try_from(make_header(message)).unwrap().into())),
        MessageDataItemName::BodyStructure => None,
        MessageDataItemName::BodyExt { section: _, partial: _, peek: _ } => {
            let data = &get_message(message["id"].as_u64().unwrap() as u32)["content"];
            let contents = data.as_str().unwrap();
            let has_attachments = message["files"].as_array().unwrap().len() > 0;
            let full_email = if has_attachments {
                make_header(message) + "\r\nContent-Type: multipart/mixed; boundary=\"=PARTLIMIT\"\r\n\r\n"
                    + "--=PARTLIMIT\r\nContent-Disposition: inline\r\nContent-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\n" + contents
                    + &message["files"].as_array().unwrap()
                        .iter()
                        .map(|attachment| {
                            let name = attachment["libelle"].as_str().unwrap();
                            let data = &get_attachment(attachment["id"].as_u64().unwrap() as u32);
                            let content_type = data.sniff_mime_type().unwrap_or("application/octet-stream");
                            format!("\r\n\r\n--=PARTLIMIT\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Type: {content_type}; name=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\nContent-Description: {name}\r\n\r\n") + &base64::encode(data)
                        })
                        .collect::<Vec<_>>()
                        .join("")
                    + "\r\n--=PARTLIMIT\r\n"
            } else {
                make_header(message) + "\r\nContent-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\n" + contents
            };
            Some(MessageDataItem::BodyExt {
                data: NString::try_from(full_email).unwrap(),
                origin: None,
                section: None,
            })
        },
        _ => todo!("item {:?} message {:?}", item, message),
    }
}

pub fn handle<'a, F: Fn(u32) -> serde_json::Value, G: Fn(u32) -> bytes::Bytes>(tag: Tag<'a>, sequence_set: SequenceSet, macro_or_item_names: MacroOrMessageDataItemNames, uid: bool, messages: Vec<(u32, serde_json::Value)>, get_message: F, get_attachment: G) -> Vec<Response<'a>> {
    let mut responses: Vec<Response> = messages
        .iter()
        .enumerate()
        .filter_map(|(pos, message)| {
            let id = if uid { message.0 } else { (pos + 1) as u32 };
            let good = sequence_set.0
                .clone()
                .into_iter()
                .map(|sequence| {
                    match sequence {
                        Sequence::Single(seq) => match seq {
                            SeqOrUid::Asterisk => true,
                            SeqOrUid::Value(v) => v.get() == id,
                        },
                        Sequence::Range(start, end) => {
                            (match start {
                                SeqOrUid::Asterisk => true,
                                SeqOrUid::Value(v) => v.get() <= id,
                            }) && (match end {
                                SeqOrUid::Asterisk => true,
                                SeqOrUid::Value(v) => id <= v.get(),
                            })
                        }
                    }
                }).any(|x| x);
            if good {
                let mut items = match &macro_or_item_names {
                    Macro(macro_name) => macro_name.expand(),
                    MessageDataItemNames(items) => items.to_vec(),
                };
                if uid {
                    items.push(MessageDataItemName::Uid);
                }
                Some(Response::Data(Data::fetch(NonZeroU32::new((pos + 1) as u32).unwrap(),
                    NonEmptyVec::try_from(items
                        .iter()
                        .filter_map(|item| { get_item(&item, &message.1, &get_message, &get_attachment) })
                        .collect::<Vec<_>>()
                    ).unwrap()).unwrap()))
            } else {
                None
            }
        })
        .collect();

    responses.push(Response::Status(
        Status::ok(Some(tag), None, "FETCH completed").unwrap(),
    ));

    responses
}

