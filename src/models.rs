use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct ProxmoxData<T> {
    #[serde(bound(deserialize = "for<'a> T: Deserialize<'a>"))]
    pub data: T,
}
