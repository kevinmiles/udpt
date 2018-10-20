use std::collections::HashMap;
use std::sync::Arc;

use actix_web;
use binascii;

use tracker;

const SERVER: &str = concat!("udpt/", env!("CARGO_PKG_VERSION"));

pub struct WebServer;

mod http_responses {
    use std;
    use binascii;
    use serde;

    #[derive(Serialize)]
    pub struct TorrentInfo {
        pub is_flagged: bool,
        pub leecher_count: u32,
        pub seeder_count: u32,
        pub completed: u32,
    }

    #[derive(Serialize)]
    pub struct TorrentList {
        pub offset: u32,
        pub length: u32,
        pub total: u32,
        #[serde(serialize_with = "infohash_as_str")]
        pub torrents: Vec<[u8; 20]>,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum APIResponse {
        Error(String),
        TorrentList(TorrentList),
        TorrentInfo(TorrentInfo),
    }

    fn infohash_as_str<S: serde::Serializer>(field: &Vec<[u8; 20]>, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let mut output_str = [0u8; 40];

        let mut seq = serializer.serialize_seq(Some(field.len()))?;

        for infohash in field.iter() {
            let _ = binascii::bin2hex(infohash, &mut output_str);

            let mystr = std::str::from_utf8(&output_str).unwrap();
            seq.serialize_element(mystr)?;
        }

        seq.end()
    }
}

struct UdptState {
    // k=token, v=username.
    access_tokens: HashMap<String, String>,
    tracker: Arc<tracker::TorrentTracker>,
}

impl UdptState {
    fn new(tracker: Arc<tracker::TorrentTracker>) -> UdptState {
        let mut tokens = HashMap::new();
        tokens.insert(String::from("h311o"), String::from("naim"));
        UdptState{
            tracker,
            access_tokens: tokens,
        }
    }
}

#[derive(Debug)]
struct UdptRequestState {
    current_user: Option<String>,
}

impl Default for UdptRequestState {
    fn default() -> Self {
        UdptRequestState{
            current_user: Option::None,
        }
    }
}

impl UdptRequestState {
    fn get_user<S>(req: &actix_web::HttpRequest<S>) -> Option<String> {
        let exts = req.extensions();
        let req_state: Option<&UdptRequestState> = exts.get();
        match req_state {
            None => None,
            Option::Some(state) => {
                match state.current_user {
                    Option::Some(ref v) => Option::Some(v.clone()),
                    None => None,
                }
            }
        }
    }
}

struct UdptMiddleware;

impl actix_web::middleware::Middleware<UdptState> for UdptMiddleware {
    fn start(&self, req: &actix_web::HttpRequest<UdptState>) -> actix_web::Result<actix_web::middleware::Started> {
        let mut req_state = UdptRequestState::default();
        if let Option::Some(token) = req.query().get("token") {
            let app_state : &UdptState = req.state();
            if let Option::Some(v) = app_state.access_tokens.get(token) {
                req_state.current_user = Option::Some(v.clone());
            }
        }
        req.extensions_mut().insert(req_state);
        Ok(actix_web::middleware::Started::Done)
    }

    fn response(&self, req: &actix_web::HttpRequest<UdptState>, mut resp: actix_web::HttpResponse) -> actix_web::Result<actix_web::middleware::Response> {
        resp.headers_mut()
            .insert(actix_web::http::header::SERVER, actix_web::http::header::HeaderValue::from_static(SERVER));

        Ok(actix_web::middleware::Response::Done(resp))
    }
}

impl WebServer {
    pub fn new(tracker: Arc<tracker::TorrentTracker>) -> WebServer {
        let server = actix_web::server::HttpServer::new(move || {
            actix_web::App::<UdptState>::with_state(UdptState::new(tracker.clone()))
                .middleware(UdptMiddleware)
                .resource("/t", |r| r.f(Self::view_torrent_list))
                .scope(r"/t/{info_hash:[\dA-Fa-f]{40,40}}", |scope| {
                    scope
                        .resource("", |r| {
                            r.method(actix_web::http::Method::GET).f(Self::view_torrent_stats);
                            r.method(actix_web::http::Method::POST).f(Self::torrent_action);
                        })
                })
                .resource("/", |r| r.method(actix_web::http::Method::GET).f(Self::view_root))
        });

        match server.bind("0.0.0.0:1212") {
            Ok(v) => {
                v.run();
            },
            Err(_) => {
                eprintln!("failed to bind server");
            }
        }

        WebServer{}
    }

    fn view_root(req: &actix_web::HttpRequest<UdptState>) -> actix_web::HttpResponse {
        actix_web::HttpResponse::build(actix_web::http::StatusCode::OK)
            .content_type("text/html")
            .body(r#"Powered by <a href="https://github.com/naim94a/udpt">https://github.com/naim94a/udpt</a>"#)
    }

    fn view_torrent_list(req: &actix_web::HttpRequest<UdptState>) -> impl actix_web::Responder {
        use std::str::FromStr;

        if UdptRequestState::get_user(req).is_none() {
            return actix_web::Json(http_responses::APIResponse::Error(String::from("access_denied")));
        }

        let req_offset = match req.query().get("offset") {
            None => 0,
            Some(v) => {
                match u32::from_str(v.as_str()) {
                    Ok(v) => v,
                    Err(_) => 0,
                }
            }
        };

        let mut req_limit = match req.query().get("limit") {
            None => 0,
            Some(v) => {
                match u32::from_str(v.as_str()) {
                    Ok(v) => v,
                    Err(_) => 0,
                }
            }
        };

        if req_limit > 4096 {
            req_limit = 4096;
        } else if req_limit == 0 {
            req_limit = 1000;
        }

        let app_state: &UdptState = req.state();
        let app_db = app_state.tracker.get_database();

        let total = app_db.len() as u32;

        let mut torrents = Vec::with_capacity(req_limit as usize);

        for (info_hash, _) in app_db.iter().skip(req_offset as usize).take(req_limit as usize) {
            torrents.push(info_hash.clone());
        }

        actix_web::Json(http_responses::APIResponse::TorrentList(http_responses::TorrentList{
            total,
            length: torrents.len() as u32,
            offset: req_offset,
            torrents,
        }))
    }

    fn view_torrent_stats(req: &actix_web::HttpRequest<UdptState>) -> actix_web::HttpResponse {
        use actix_web::FromRequest;

        if UdptRequestState::get_user(req).is_none() {
            return actix_web::HttpResponse::build(actix_web::http::StatusCode::UNAUTHORIZED)
                .json(http_responses::APIResponse::Error(String::from("access_denied")));
        }

        let path: actix_web::Path<String> = match actix_web::Path::extract(req) {
            Ok(v) => v,
            Err(_) => {
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR)
                    .json(http_responses::APIResponse::Error(String::from("internal_error")));
            }
        };

        let mut info_hash = [0u8; 20];
        if let Err(_) = binascii::hex2bin((*path).as_bytes(), &mut info_hash) {
            return actix_web::HttpResponse::build(actix_web::http::StatusCode::BAD_REQUEST)
                .json(http_responses::APIResponse::Error(String::from("invalid_info_hash")));
        }

        let app_state: &UdptState = req.state();

        let db = app_state.tracker.get_database();
        let entry = match db.get(&info_hash) {
            Some(v) => v,
            None => {
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::NOT_FOUND)
                    .json(http_responses::APIResponse::Error(String::from("not_found")));
            }
        };

        let is_flagged = entry.is_flagged();
        let (seeders, completed, leechers) = entry.get_stats();

        return actix_web::HttpResponse::build(actix_web::http::StatusCode::OK)
            .json(http_responses::APIResponse::TorrentInfo(
                http_responses::TorrentInfo{
                    is_flagged,
                    seeder_count: seeders,
                    leecher_count: leechers,
                    completed,
                }
            ));
    }

    fn torrent_action(req: &actix_web::HttpRequest<UdptState>) -> actix_web::HttpResponse {
        use actix_web::FromRequest;

        if UdptRequestState::get_user(req).is_none() {
            return actix_web::HttpResponse::build(actix_web::http::StatusCode::UNAUTHORIZED)
                .json(http_responses::APIResponse::Error(String::from("access_denied")));
        }

        let query = req.query();
        let action_opt = query.get("action");
        let action = match action_opt {
            Some(v) => v,
            None => {
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::BAD_REQUEST)
                    .json(http_responses::APIResponse::Error(String::from("action_required")));
            }
        };

        let app_state: &UdptState = req.state();

        let path: actix_web::Path<String> = match actix_web::Path::extract(req) {
            Ok(v) => v,
            Err(_) => {
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR)
                    .json(http_responses::APIResponse::Error(String::from("internal_error")));
            }
        };

        let mut info_hash = [0u8; 20];
        if let Err(_) = binascii::hex2bin((*path).as_bytes(), &mut info_hash) {
            return actix_web::HttpResponse::build(actix_web::http::StatusCode::BAD_REQUEST)
                .json(http_responses::APIResponse::Error(String::from("invalid_info_hash")));
        }

        match action.as_str() {
            "flag" => {
                app_state.tracker.set_torrent_flag(&info_hash, true);
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::OK)
                    .body("")
            },
            "unflag" => {
                app_state.tracker.set_torrent_flag(&info_hash, false);
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::OK)
                    .body("")
            },
            "add" => {
                let success = app_state.tracker.add_torrent(&info_hash).is_ok();
                let code = if success { actix_web::http::StatusCode::OK } else { actix_web::http::StatusCode::INTERNAL_SERVER_ERROR };

                return actix_web::HttpResponse::build(code)
                    .body("")
            },
            "remove" => {
                let success = app_state.tracker.remove_torrent(&info_hash, true).is_ok();
                let code = if success { actix_web::http::StatusCode::OK } else { actix_web::http::StatusCode::INTERNAL_SERVER_ERROR };

                return actix_web::HttpResponse::build(code)
                    .body("")
            },
            _ => {
                return actix_web::HttpResponse::build(actix_web::http::StatusCode::BAD_REQUEST)
                    .json(http_responses::APIResponse::Error(String::from("invalid_action")));
            }
        }
    }
}
