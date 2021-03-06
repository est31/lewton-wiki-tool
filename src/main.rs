extern crate cmp;
extern crate crossbeam_channel;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

extern crate hyper;
extern crate openssl;
extern crate hyper_openssl;
extern crate tokio;
extern crate md5;
extern crate percent_encoding;

extern crate structopt;
extern crate chrono;
extern crate pbr;

use std::io::Cursor;
use std::io::{Read, SeekFrom, Seek, Write, BufRead, BufReader};
use std::fs::{File, OpenOptions};
use std::collections::HashMap;

use hyper::{Client, Request, Body};
use hyper::header::USER_AGENT;
use hyper::rt::{Future, Stream};
use hyper::client::connect::{HttpConnector, Connect};
use hyper_openssl::HttpsConnector;
use openssl::ssl::{SslMethod, SslConnector};

use tokio::runtime::Runtime;

use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};

use structopt::StructOpt;

use crossbeam_channel::Sender;

use tokio::prelude::future::{ok, Either};
use tokio::runtime::Builder as RuntimeBuilder;

use serde_json::{to_string, from_str};

use chrono::DateTime;
use chrono::offset::Utc;

use pbr::ProgressBar;

// user agent to use
const AGENT :&str = "LewtonWikiTool (https://github.com/est31/lewton-wiki-tool) Hyper/0.12";

#[derive(StructOpt, Debug)]
enum Options {
	#[structopt(name = "get")]
	Get {
		name :String,
	},
	#[structopt(name = "get-list")]
	GetList {
		list_path :String,
		#[structopt(short = "j", long = "jobs")]
		jobs :Option<usize>,
		#[structopt(short = "l", long = "logfile")]
		logfile :Option<String>,
		#[structopt(short = "s", long = "no-log-to-stdout",
			help = "Suppresses logging of the results to stdout")]
		no_log_to_stdout :bool,
		#[structopt(short = "p", long = "progress",
			help = "Displays a progress bar, implies -s")]
		progress_bar :bool,
		#[structopt(short = "r", long = "requests",
			help = "Upper limit of parallel requests, default 20")]
		requests :Option<u32>,
	},
	#[structopt(name = "show-url")]
	ShowUrl {
		name :String,
	},
}

#[derive(Debug)]
struct StrErr(String);

use std::fmt::Display;
impl<T :Display> From<T> for StrErr {
	fn from(v :T) -> Self {
		StrErr(format!("{}", v))
	}
}

fn main() -> Result<(), StrErr> {
	let options = Options::from_args();

	match options {
		Options::ShowUrl { name } => {
			let url = get_medium_url(&name);
			println!("URL is: {}", url);
		},
		Options::Get { name } => {
			let client = create_client()?;

			let mut runtime = Runtime::new().unwrap();

			let url = get_medium_url(&name);
			let url = url.parse::<hyper::Uri>().unwrap();
			runtime.spawn(fetch_url_verbose(&client, url));

			runtime.shutdown_on_idle()
				.wait().unwrap();
		},
		Options::GetList { list_path, jobs, logfile,
				no_log_to_stdout, progress_bar, requests } => {
			let no_log_to_stdout = no_log_to_stdout || progress_bar;
			let requests = requests.unwrap_or(20);

			println!("opening list file {}", list_path);
			let f = File::open(list_path)?;

			let (mut log_file, prior_log_entries) = if let Some(p) = logfile {
				println!("opening log file {}", p);
				let mut f = OpenOptions::new()
					.read(true)
					.write(true)
					.create(true)
					.open(&p)?;
				let prior_log_entries = parse_result_map(&f)?;
				f.seek(SeekFrom::End(0))?;
				(Some(f), prior_log_entries)
			} else {
				(None, HashMap::new())
			};
			let mut br = BufReader::new(f);
			let mut rt_build = RuntimeBuilder::new();
			if let Some(j) = jobs {
				rt_build.core_threads(j);
			}
			let mut runtime = rt_build.build()?;

			let client = create_client()?;

			let (s, r) = crossbeam_channel::unbounded();

			let mut names :Vec<String> = Vec::new();
			for l in br.lines() {
				names.push(l?);
			}
			let prior_final_count = prior_log_entries
				.iter()
				.filter(|(_, v)| v.iter().any(RequestRes::is_final))
				.count();
			let left_to_handle = names.len() - prior_final_count;
			println!("Total URL count: {}", names.len());
			println!("Number of already handled URLs: {}", prior_final_count);
			println!("URLs left to handle: {}", left_to_handle);

			let mut pb = if progress_bar {
				Some(ProgressBar::new(left_to_handle as u64))
			} else {
				None
			};

			let mut offs = 0;
			let mut handled = 0;
			for name in names.iter() {
				offs += 1;
				// If we've already gotten a "final" result for the file,
				// skip it.
				if Some(true) == prior_log_entries
						.get(name)
						.map(|v| v.iter().any(RequestRes::is_final)) {
					continue;
				}
				runtime.spawn(fetch_name(&client, name.to_string(), s.clone()));

				if handled + 1 == requests as usize {
					break;
				}
				handled += 1;
			}

			let mut s_opt = Some(s);

			if left_to_handle == 0 {
				s_opt.take();
			}

			while let Ok(msg) =  r.recv() {
				if let Some(ref mut pb) = &mut pb {
					pb.inc();
				}
				if !no_log_to_stdout {
					println!("{:?}", msg);
				}
				if let Some(ref mut lf) = &mut log_file {
					let msg_str = to_string(&msg).unwrap();
					writeln!(lf, "{}", msg_str);
				}
				let name = names.get(offs);
				offs += 1;
				if let Some(name) = name {
					// If we've already gotten a "final" result for the file,
					// skip it.
					if Some(true) != prior_log_entries
							.get(name)
							.map(|v| v.iter().any(RequestRes::is_final)) {
						runtime.spawn(fetch_name(&client, name.to_string(), s_opt.as_ref().unwrap().clone()));
					}
				} else if s_opt.is_some() {
					s_opt.take();
				}
			}
			pb.map(|mut pb| pb.finish_print("done"));
			println!();

			runtime.shutdown_now()
				.wait().map_err(|_| "couldn't shut down the runtime")?;

			// I would actually use this, but it hangs indefinitely
			// if it had something to do, even if every task already
			// finished.
			//runtime.shutdown_on_idle()
			//	.wait().map_err(|_| "couldn't shut down the runtime")?;
		}
	}
	Ok(())
}


#[derive(Debug, Serialize, Deserialize)]
struct RequestRes {
	/// Filename that was requested
	file_name :String,
	/// Time of entry
	entry_time :DateTime<Utc>,
	/// Result payload
	result_kind :RequestResKind,
}

#[derive(Debug, Serialize, Deserialize)]
enum RequestResKind {
	/// Successful response, and comparison ran without producing a fatal error
	/// (Mismatches still possible).
	Success(ComparisonRes),
	/// Successful response, but comparison produced an error.
	///
	/// Note that this is different from a mismatch.
	/// Errors occur e.g. when the stream is invalid or not vorbis or something.
	ComparisonErr(String),
	/// Got a response but with wrong status code
	WrongResponse(u16),
	/// Error during obtaining a response
	Error(String),
}

impl RequestRes {
	/// Returns whether the given instance is "final", as in not to be
	/// changed.
	fn is_final(&self) -> bool {
		match self.result_kind {
			// Network errors, etc.
			RequestResKind::Error(_) => false,
			// Anything else we consider as a final reply.
			_ => true,
		}
	}
}

#[derive(Debug, Serialize, Deserialize)]
struct ComparisonRes {
	n :(usize, usize, usize, bool, u32, u8),
	vendor :String,
}

#[allow(unused)]
impl ComparisonRes {
	fn pck_issues(&self) -> usize {
		self.n.0
	}
	fn pck_total(&self) -> usize {
		self.n.1
	}
	fn sample_count(&self) -> usize {
		self.n.2
	}
	fn chained(&self) -> bool {
		self.n.3
	}
	fn sample_rate(&self) -> u32 {
		self.n.4
	}
	fn channel_count(&self) -> u8 {
		self.n.5
	}
}

fn parse_result_map<R :Read>(rdr :R) -> Result<HashMap<String, Vec<RequestRes>>, StrErr> {
	let buf_rdr = BufReader::new(rdr);
	let mut res = HashMap::<_, Vec<RequestRes>>::new();
	for l in buf_rdr.lines() {
		let json :RequestRes = from_str(&l?)?;
		res.entry(json.file_name.clone()).or_default().push(json);
	}
	Ok(res)
}

fn get_medium_url(name :&str) -> String {
	let hash = md5::compute(name.as_bytes());
	let hash_hex = format!("{:x}", hash);

	let name_percent_encoded = utf8_percent_encode(name, DEFAULT_ENCODE_SET);
	let url = format!("https://upload.wikimedia.org/wikipedia/commons/{}/{}/{}",
		&hash_hex[..1], &hash_hex[..2], name_percent_encoded);
	url
}

/// Creates a client that performs https requests
fn create_client() -> Result<Client<HttpsConnector<HttpConnector>>, StrErr> {
	let mut ssl = SslConnector::builder(SslMethod::tls())?;
	ssl.set_alpn_protos(b"\x02h2")?;
	let mut http = HttpConnector::new(4);
	http.enforce_http(false);
	let https = HttpsConnector::with_connector(http, ssl)?;

	let client = Client::builder()
		.http2_only(true)
		.build::<_, hyper::Body>(https);
	Ok(client)
}

fn fetch_url_verbose<T :'static + Sync + Connect>(client :&Client<T>, url :hyper::Uri) -> impl Future<Item=(), Error=()> {

	let mut req = Request::builder();
	req.uri(url)
		.header(USER_AGENT, AGENT);

	client
		.request(req.body(Body::empty()).unwrap())
		.and_then(|res| {
			println!("Response: {} success: {}", res.status(), res.status().is_success());
			println!("Response version: {:?}", res.version());
			println!("Headers: {:#?}", res.headers());

			let is_success = res.status().is_success();
			res.into_body().concat2().map(move |body| {
				if is_success {
					println!("body is {} characters long", body.len());
					let cursor1 = Cursor::new(&body);
					let cursor2 = Cursor::new(&body);
					let res = cmp::cmp_output(cursor1, cursor2,
						|pck_issues, pck_total, samples_total, chained, id, _cmt, _setup| {
							(pck_issues, pck_total, samples_total, chained, id.audio_channels)
						});
					println!("Comparison result: {:?}", res);
				}
			})
		})
		.map(|_| {
			println!("\n\nDone.");
		})
		.map_err(|err| {
			eprintln!("Error {}", err);
		})
}

fn fetch_name<T :'static + Sync + Connect>(client :&Client<T>, name :String, sender :Sender<RequestRes>) -> impl Future<Item=(), Error=()> {

	let url = get_medium_url(&name);
	let url = url.parse::<hyper::Uri>().unwrap();

	let mut req = Request::builder();

	req.uri(url)
		.header(USER_AGENT, AGENT);

	let send_kind = move |result_kind| {
		let _  = sender.send(RequestRes {
			file_name : name,
			entry_time : Utc::now(),
			result_kind,
		});
	};
	client
		.request(req.body(Body::empty()).unwrap())
		.then(move |res| {
			match res {
				Ok(res) => {
					let status = res.status();
					Either::A(res.into_body().concat2().map(move |body| {
						if status.is_success() {
							let cursor1 = Cursor::new(&body);
							let cursor2 = Cursor::new(&body);
							let res = cmp::cmp_output(cursor1, cursor2,
								|pck_issues, pck_total, sample_count, chained, id, cmt, _setup| {
									ComparisonRes {
										n : (pck_issues,
											pck_total,
											sample_count,
											chained,
											id.audio_sample_rate,
											id.audio_channels,),
										vendor : cmt.vendor.clone(),
									}
								});
							let kind = match res {
								Ok(r) => RequestResKind::Success(r),
								Err(s) => RequestResKind::ComparisonErr(s),
							};
							send_kind(kind);
						} else {
							send_kind(RequestResKind::WrongResponse(status.as_u16()));
						}
					}))
				},
				Err(err) => {
					send_kind(RequestResKind::Error(format!("{}", err)));
					Either::B(ok(()))
				},
			}
		})
		.map_err(|_err| {})
}
