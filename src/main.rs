use std::{
    env,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use ed25519_dalek::{PublicKey, Verifier, PUBLIC_KEY_LENGTH};
use hex::FromHex;
use hyper::{
    header::CONTENT_TYPE,
    service::{make_service_fn, service_fn},
    Body,
    Method,
    Request,
    Response,
    Server,
    StatusCode,
};
use lib_2048::{BoardSpace, GameBoard, MoveDirection, GAME_BOARD_SIZE};
use once_cell::sync::Lazy;
use serde::Serialize;
use twilight_model::{
    application::{
        component::{
            button::ButtonStyle,
            ActionRow,
            Button,
            Component,
            ComponentType,
        },
        interaction::{
            application_command::{CommandData, CommandOptionValue},
            message_component::MessageComponentInteractionData,
            Interaction,
            InteractionData,
            InteractionType,
        },
    },
    channel::{message::MessageFlags, ReactionType},
    http::interaction::{
        InteractionResponse,
        InteractionResponseData,
        InteractionResponseType,
    },
};

const ZWS: &str = "​";

static PUB_KEY: Lazy<PublicKey> = Lazy::new(|| {
    let pk = env::var("DISCORD_PUBLIC_KEY")
        .expect("Missing `DISCORD_PUBLIC_KEY` in env");

    PublicKey::from_bytes(
        &<[u8; PUBLIC_KEY_LENGTH] as FromHex>::from_hex(pk).unwrap(),
    )
    .unwrap()
});

#[tokio::main]
async fn main() {
    let port = env::var("PORT")
        .unwrap_or("3000".to_string())
        .parse::<u16>()
        .unwrap();
    let addr = env::var("ADDR").unwrap_or("0.0.0.0".into());

    let addr = SocketAddr::new(IpAddr::from_str(&addr).unwrap(), port);

    let service = make_service_fn(|_| async {
        Ok::<_, anyhow::Error>(service_fn(|req| interaction_handler(req)))
    });

    println!("Starting server on addr={:?}", &addr);
    Server::bind(&addr).serve(service).await.unwrap();
}

fn empty_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}

fn json_response<T: Serialize>(payload: &T) -> Response<Body> {
    let body = serde_json::to_vec(payload).unwrap();

    return Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(body.into())
        .unwrap();
}

async fn interaction_handler(
    req: Request<Body>,
) -> anyhow::Result<Response<Body>> {
    if req.method() != Method::POST {
        return Ok(empty_response(StatusCode::METHOD_NOT_ALLOWED));
    }

    if req.uri().path() != "/i" {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    }

    let timestamp =
        if let Some(timestamp) = req.headers().get("x-signature-timestamp") {
            timestamp.to_owned()
        } else {
            return Ok(empty_response(StatusCode::BAD_REQUEST));
        };

    let sig = if let Some(timestamp) = req.headers().get("x-signature-ed25519")
    {
        timestamp.to_str()?.parse()?
    } else {
        return Ok(empty_response(StatusCode::BAD_REQUEST));
    };

    let body = hyper::body::to_bytes(req).await?;

    if PUB_KEY
        .verify(vec![timestamp.as_bytes(), &body].concat().as_ref(), &sig)
        .is_err()
    {
        return Ok(empty_response(StatusCode::BAD_REQUEST));
    }

    let body = serde_json::from_slice::<Interaction>(&body)?;

    match body.kind {
        InteractionType::Ping => {
            let response = InteractionResponse {
                kind: InteractionResponseType::Pong,
                data: None,
            };

            let json = serde_json::to_vec(&response)?;
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json")
                .body(json.into())?);
        },

        InteractionType::ApplicationCommand => {
            let data = match body.data.as_ref() {
                Some(InteractionData::ApplicationCommand(data)) => data,
                _ => unreachable!(),
            };

            let response = handle_command(&body, &data).await?;
            return Ok(json_response(&response));
        },

        InteractionType::MessageComponent => {
            let data = match &body.data {
                Some(InteractionData::MessageComponent(data)) => data,
                _ => unreachable!(),
            };

            if data.component_type == ComponentType::Button {
                let response = handle_button(&body, &data);
                return Ok(json_response(&response));
            }
        },

        _ => {},
    }

    return Ok(empty_response(StatusCode::BAD_REQUEST));
}

async fn handle_command(
    _i: &Interaction,
    data: &CommandData,
) -> anyhow::Result<InteractionResponse> {
    match data.name.as_str() {
        "2048" => {
            let ephemeral = {
                if let Some(opt) =
                    data.options.iter().find(|o| o.name == "ephemeral")
                {
                    if let CommandOptionValue::Boolean(b) = opt.value {
                        b
                    } else {
                        true
                    }
                } else {
                    true
                }
            };

            let game = GameBoard::new();

            let resp = InteractionResponse {
                kind: InteractionResponseType::ChannelMessageWithSource,
                data: Some(InteractionResponseData {
                    content: Some(format!("**Score:** {}", &game.score)),
                    components: Some(game_board_to_msg_components(&game)),
                    flags: if ephemeral {
                        Some(MessageFlags::EPHEMERAL)
                    } else {
                        None
                    },
                    ..Default::default()
                }),
            };

            return Ok(resp);
        },

        "invite" => {
            return Ok(InteractionResponse {
                kind: InteractionResponseType::ChannelMessageWithSource,
                data: Some(InteractionResponseData {
                    content: Some(
                        "https://discord.com/api/oauth2/authorize?client_id=1025178745578991696&scope=applications.commands".into(),
                    ),
                    flags: Some(MessageFlags::EPHEMERAL),
                    ..Default::default()
                }),
            });
        },

        _ => {
            unreachable!()
        },
    }
}

fn handle_button(
    i: &Interaction,
    data: &MessageComponentInteractionData,
) -> InteractionResponse {
    let mut game = {
        let msg = i.message.as_ref().unwrap();

        let component_labels = msg.components[0..4]
            .iter()
            .map(|c| match c {
                Component::ActionRow(ar) => ar
                    .components
                    .iter()
                    .map(|c2| match &c2 {
                        Component::Button(b) => {
                            match b.label.as_ref().unwrap().as_str() {
                                ZWS => BoardSpace::Vacant,
                                t @ _ => {
                                    let num = t.parse::<usize>().unwrap();
                                    BoardSpace::Tile(num)
                                },
                            }
                        },
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>(),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();

        let score = {
            const SCORE_PREFIX: &str = "**Score:** ";
            if let Some(start) = msg.content.find(SCORE_PREFIX) {
                let after = &msg.content[(start + SCORE_PREFIX.len())..]
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(0);
                *after
            } else {
                0
            }
        };

        let mut a = GameBoard::empty();
        a.score = score;

        // TODO: is there a better way to turn Vec<Vec<T>> into [[T;4];4]
        for y in 0..GAME_BOARD_SIZE {
            for x in 0..GAME_BOARD_SIZE {
                a.cells[y][x] = component_labels[y][x];
            }
        }
        a
    };

    // there will always be a message and interaction present
    let msg = i.message.as_ref().unwrap();
    let int = msg.interaction.as_ref().unwrap();

    if i.author_id().unwrap() != int.user.id {
        let resp = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(InteractionResponseData {
                content: Some(
                    "Not your game! Start one by running `/2048`".to_string(),
                ),
                flags: Some(MessageFlags::EPHEMERAL),
                ..Default::default()
            }),
        };

        return resp;
    }

    match data.custom_id.as_str() {
        "up" => game.r#move(MoveDirection::Up),
        "down" => game.r#move(MoveDirection::Down),
        "left" => game.r#move(MoveDirection::Left),
        "right" => game.r#move(MoveDirection::Right),

        _ => {
            let resp = InteractionResponse {
                kind: InteractionResponseType::DeferredUpdateMessage,
                data: None,
            };
            return resp;
        },
    }

    let resp = InteractionResponse {
        kind: InteractionResponseType::UpdateMessage,
        data: Some(InteractionResponseData {
            content: Some(
                if game.has_lost() {
                    format!("**Game Over!**\n> **Score:** {}", game.score)
                } else {
                    format!("**Score:** {}", game.score)
                },
            ),
            components: Some(game_board_to_msg_components(&game)),
            ..Default::default()
        }),
    };

    return resp;
}

fn game_board_to_msg_components(board: &GameBoard) -> Vec<Component> {
    let mut rows = board
        .cells
        .iter()
        .enumerate()
        .map(|(y, row)| {
            Component::ActionRow(ActionRow {
                components: {
                    row.iter()
                        .enumerate()
                        .map(|(x, cell)| {
                            let is_vacant = cell == &BoardSpace::Vacant;

                            Component::Button(Button {
                                url: None,
                                emoji: None,
                                disabled: is_vacant,
                                custom_id: Some(format!("{x}-{y}")),
                                style: match cell {
                                    &BoardSpace::Tile(t) if t >= 2048 => {
                                        ButtonStyle::Success
                                    },
                                    &BoardSpace::Tile(_) => {
                                        ButtonStyle::Primary
                                    },
                                    &BoardSpace::Vacant => {
                                        ButtonStyle::Secondary
                                    },
                                },
                                label: Some({
                                    if let BoardSpace::Tile(num) = cell {
                                        num.to_string()
                                    } else {
                                        ZWS.to_string()
                                    }
                                }),
                            })
                        })
                        .collect::<Vec<_>>()
                },
            })
        })
        .collect::<Vec<_>>();

    let controls = Component::ActionRow(ActionRow {
        components: {
            [["left", "⬅️"], ["up", "⬆️"], ["down", "⬇️"], ["right", "➡️"]]
                .iter()
                .map(|c| {
                    Component::Button(Button {
                        custom_id: Some(c[0].to_string()),
                        disabled: board.has_lost(),
                        emoji: Some(ReactionType::Unicode {
                            name: c[1].to_string(),
                        }),
                        label: None,
                        style: ButtonStyle::Success,
                        url: None,
                    })
                })
                .collect::<Vec<_>>()
        },
    });

    rows.push(controls);

    rows
}
