use super::*;

// --- the transport and security fields every scheme spells the same way ---

/// The half of a link that decides *how* the connection is carried, gathered
/// from a query string or from a vmess object's own field names.
#[derive(Debug, Default)]
pub(super) struct Transport {
    pub(super) network: Option<String>,
    pub(super) path: Option<String>,
    pub(super) host: Option<String>,
    pub(super) service_name: Option<String>,
    pub(super) security: Option<String>,
    pub(super) server_name: Option<String>,
    pub(super) fingerprint: Option<String>,
    pub(super) alpn: Option<String>,
    pub(super) public_key: Option<String>,
    pub(super) short_id: Option<String>,
    pub(super) spider_x: Option<String>,
    pub(super) header_type: Option<String>,
    pub(super) plugin: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) flow: Option<String>,
    pub(super) encryption: Option<String>,
}

impl Transport {
    pub(super) fn from_pairs<'a>(
        pairs: impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)>,
    ) -> Self {
        let mut transport = Self::default();
        for (key, value) in pairs {
            let value = value.trim().to_string();
            if value.is_empty() {
                continue;
            }
            match key.trim().to_ascii_lowercase().as_str() {
                "type" | "network" => transport.network = Some(value),
                "path" => transport.path = Some(value),
                "host" => transport.host = Some(value),
                "servicename" | "service-name" | "service_name" => {
                    transport.service_name = Some(value)
                }
                "security" => transport.security = Some(value),
                "sni" | "peer" | "servername" | "server_name" => {
                    transport.server_name = Some(value)
                }
                "fp" | "fingerprint" | "client-fingerprint" => transport.fingerprint = Some(value),
                "alpn" => transport.alpn = Some(value),
                "pbk" | "publickey" | "public-key" => transport.public_key = Some(value),
                "sid" | "shortid" | "short-id" => transport.short_id = Some(value),
                "spx" | "spiderx" | "spider-x" => transport.spider_x = Some(value),
                "headertype" | "header-type" => transport.header_type = Some(value),
                "plugin" => transport.plugin = Some(value),
                "mode" => transport.mode = Some(value),
                "flow" => transport.flow = Some(value),
                "encryption" => transport.encryption = Some(value),
                // Anything else is another client's business.
                _ => {}
            }
        }
        transport
    }

    pub(super) fn stream(
        &self,
        scheme: &'static str,
        default_security: StreamSecurity,
    ) -> Result<StreamSettings, UriError> {
        self.refuse_what_this_program_cannot_carry()?;

        let network = match lower(&self.network).as_deref() {
            None | Some("tcp") => StreamNetwork::Tcp,
            Some("ws") | Some("websocket") => StreamNetwork::Ws,
            Some("grpc") | Some("gun") => StreamNetwork::Grpc,
            Some(other) => {
                return Err(UriError::Unsupported {
                    field: "type",
                    value: other.to_string(),
                    reason: "only tcp, ws and grpc are modelled",
                });
            }
        };

        let security = match lower(&self.security).as_deref() {
            None => default_security,
            Some("none") => StreamSecurity::None,
            Some("tls") => StreamSecurity::Tls,
            Some("reality") => StreamSecurity::Reality,
            Some(other) => {
                return Err(UriError::Unsupported {
                    field: "security",
                    value: other.to_string(),
                    reason: "only none, tls and reality are modelled",
                });
            }
        };

        let tls_asked_for =
            self.server_name.is_some() || self.fingerprint.is_some() || self.alpn.is_some();
        let reality_asked_for =
            self.public_key.is_some() || self.short_id.is_some() || self.spider_x.is_some();

        let mut stream = StreamSettings {
            network,
            security,
            ..Default::default()
        };
        match security {
            StreamSecurity::None => {
                if tls_asked_for {
                    return Err(UriError::Unsupported {
                        field: "sni",
                        value: self.server_name.clone().unwrap_or_default(),
                        reason: "the link asks for no tls, so nothing would read it",
                    });
                }
                if reality_asked_for {
                    return Err(UriError::Unsupported {
                        field: "pbk",
                        value: self.public_key.clone().unwrap_or_default(),
                        reason: "the link asks for no reality, so nothing would read it",
                    });
                }
            }
            StreamSecurity::Tls => {
                if reality_asked_for {
                    return Err(UriError::Unsupported {
                        field: "pbk",
                        value: self.public_key.clone().unwrap_or_default(),
                        reason: "the link asks for tls, not reality",
                    });
                }
                if tls_asked_for {
                    stream.tls = Some(TlsSettings {
                        server_name: self.server_name.clone(),
                        fingerprint: self.fingerprint.clone(),
                        alpn: self.alpn_list(),
                    });
                }
            }
            StreamSecurity::Reality => {
                let public_key = text(self.public_key.as_deref().unwrap_or_default()).ok_or(
                    UriError::Missing {
                        scheme,
                        field: "pbk",
                    },
                )?;
                stream.reality = Some(RealitySettings {
                    server_name: self.server_name.clone(),
                    public_key,
                    short_id: self.short_id.clone(),
                    fingerprint: self.fingerprint.clone(),
                    spider_x: self.spider_x.clone(),
                });
            }
        }

        match network {
            StreamNetwork::Tcp => {
                if self.path.is_some() || self.host.is_some() {
                    return Err(UriError::Unsupported {
                        field: "path",
                        value: self.path.clone().unwrap_or_default(),
                        reason: "the link asks for no websocket, so nothing would read it",
                    });
                }
            }
            StreamNetwork::Ws => {
                stream.ws = Some(WsSettings {
                    path: self.path.clone(),
                    host: self.host.clone(),
                });
            }
            StreamNetwork::Grpc => {
                let service_name = text(self.service_name.as_deref().unwrap_or_default()).ok_or(
                    UriError::Missing {
                        scheme,
                        field: "serviceName",
                    },
                )?;
                stream.grpc = Some(GrpcSettings { service_name });
            }
        }

        // The same rules the editor refuses a save on, so a link that parses is
        // a link that could have been typed in.
        validate_stream(&stream).map_err(|error| UriError::WouldNotLaunch {
            detail: error.to_string(),
        })?;

        Ok(stream)
    }

    /// The fields this model has nowhere to put, refused by name rather than
    /// dropped. Each one changes what the server expects to receive.
    pub(super) fn refuse_what_this_program_cannot_carry(&self) -> Result<(), UriError> {
        if let Some(plugin) = text(self.plugin.as_deref().unwrap_or_default()) {
            return Err(UriError::Unsupported {
                field: "plugin",
                value: plugin,
                reason: "this program starts Xray itself and does not run a shadowsocks plugin",
            });
        }
        if let Some(header_type) = lower(&self.header_type)
            && header_type != "none"
        {
            return Err(UriError::Unsupported {
                field: "headerType",
                value: header_type,
                reason: "tcp header obfuscation is not modelled",
            });
        }
        if let Some(mode) = lower(&self.mode)
            && mode != "gun"
        {
            return Err(UriError::Unsupported {
                field: "mode",
                value: mode,
                reason: "only the gRPC gun mode is modelled",
            });
        }
        Ok(())
    }

    pub(super) fn alpn_list(&self) -> Vec<String> {
        self.alpn
            .as_deref()
            .map(|alpn| alpn.split(',').filter_map(text).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}
