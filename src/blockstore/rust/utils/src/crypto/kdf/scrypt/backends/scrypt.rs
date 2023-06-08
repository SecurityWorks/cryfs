use anyhow::Result;
// TODO Separate out InfallibleUnwrap from lockable and don't depend on lockable from this crate
use lockable::InfallibleUnwrap;

use crate::crypto::kdf::scrypt::params::ScryptParams;
use crate::crypto::kdf::scrypt::settings::ScryptSettings;
use crate::crypto::kdf::PasswordBasedKDF;
use crate::crypto::symmetric::EncryptionKey;

pub struct ScryptScrypt;

impl PasswordBasedKDF for ScryptScrypt {
    type Settings = ScryptSettings;
    type Parameters = ScryptParams;

    fn derive_key(key_size: usize, password: &str, kdf_parameters: &ScryptParams) -> EncryptionKey {
        let params = scrypt::Params::new(
            kdf_parameters.log_n(),
            kdf_parameters.r(),
            kdf_parameters.p(),
            // scrypt::Params::len is an ignored field so shouldn't really matter what we give it
            scrypt::Params::RECOMMENDED_LEN,
        )
        .expect("Invalid scrypt parameters");
        EncryptionKey::new(key_size, |key_data| {
            Ok(scrypt::scrypt(
                password.as_bytes(),
                kdf_parameters.salt(),
                &params,
                key_data,
            )
            .expect("Error in scrypt"))
        })
        .infallible_unwrap()
    }

    fn generate_parameters(settings: &ScryptSettings) -> Result<ScryptParams> {
        ScryptParams::generate(settings)
    }
}
