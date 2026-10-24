//! Command and document text of the native stores (Redis commands, MongoDB and Elasticsearch
//! request bodies, DynamoDB calls): what Preview shows and what is sent come from here, as SQL
//! comes from the rest of `sqlgen`. One module per store, built with its driver.

#[cfg(feature = "drv-dynamodb")]
pub mod dynamo;
#[cfg(feature = "drv-elasticsearch")]
pub mod elastic;
#[cfg(feature = "drv-mongodb")]
pub mod mongo;
#[cfg(feature = "drv-redis")]
pub mod redis;
