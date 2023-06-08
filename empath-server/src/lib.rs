pub mod smtp;

use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use empath_common::{
    ffi::module::{Module, ModuleError},
    listener::Listener,
    log::Logger,
};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use thiserror::Error;
// use trust_dns_resolver::{
//     config::{ResolverConfig, ResolverOpts},
//     Resolver,
// };

#[derive(Error, Debug)]
pub enum ServerError {
    #[error(transparent)]
    ModuleError(#[from] ModuleError),

    #[error(transparent)]
    IO(#[from] std::io::Error),
}

#[derive(Serialize, Deserialize, Default)]
pub struct Server {
    listeners: Vec<Box<dyn Listener>>,
    modules: Vec<Module>,
}

impl Server {
    pub fn from_config(file: &str) -> std::io::Result<Server> {
        let file = Path::new(file);
        let mut reader = BufReader::new(File::open(file)?);
        let mut config = String::new();
        reader.read_to_string(&mut config)?;

        toml::from_str(&config).map_err(|_| std::io::ErrorKind::InvalidData.into())
    }

    /// Run the server, which will accept connections on the
    /// port it is asked to (or the default if not chosen).
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_server::Server;
    ///
    /// let server = Server::default();
    /// server.run();
    /// ```
    ///
    /// # Errors
    ///
    /// This function will return an error if there is an issue accepting a connection,
    /// or if there is an issue binding to the specific address and port combination.
    pub async fn run(self) -> Result<(), ServerError> {
        Logger::init();

        Module::init(&self.modules)?;

        join_all(self.listeners.iter().map(|listener| listener.spawn())).await;

        Ok(())
    }

    pub fn with_listener(mut self, listener: Box<dyn Listener>) -> Server {
        self.listeners.push(listener);

        self
    }

    pub fn with_module(mut self, module: Module) -> Server {
        self.modules.push(module);

        self
    }
}

// async fn forward(vctx: &ValidationContext) -> std::io::Result<()> {
//     println!("{vctx:#?}");

//     let from = vctx.mail_from.as_ref().unwrap().split(':').nth(1).unwrap();
//     let to = vctx
//         .rcpt_to
//         .as_ref()
//         .unwrap()
//         .iter()
//         .map(|to| to.split(':').nth(1).unwrap())
//         .collect::<Vec<_>>();

//     let from = if let MailAddr::Single(SingleInfo { addr, .. }) =
//         mailparse::addrparse(from).unwrap().first().unwrap()
//     {
//         addr.clone()
//     } else {
//         String::default()
//     };
//     let to = mailparse::addrparse(to.join(",").as_str()).unwrap();

//     println!("{from:#?} --> {to:#?}");

//     let resolver = Resolver::new(ResolverConfig::default(), ResolverOpts::default()).unwrap();

//     if let MailAddr::Single(SingleInfo { addr, .. }) = to.first().unwrap() {
//         let response = resolver.mx_lookup(addr.split('@').nth(1).unwrap()).unwrap();
//         let response = response.iter().next().unwrap();

//         println!("{}", response.exchange());

//         let response = resolver.lookup_ip(response.exchange().to_string()).unwrap();

//         let address = response.iter().next().expect("no addresses returned!");

//         println!("{address}");

//         let conn = Async::<TcpStream>::connect((address, 25)).await?;

//         println!("{conn:#?}");

//         let mut buffer = [0; 4096];

//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "EHLO test-local\r\n"))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "MAIL FROM:<{from}>\r\n"))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "RCPT TO:<{to}>\r\n"))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "DATA\r\n")).await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "{}\r\n", vctx.data.as_ref().unwrap()))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         conn.write_with(|mut conn| write!(conn, "QUIT\r\n")).await?;
//     }

//     Ok(())
// }
