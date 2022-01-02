/*
   Copyright 2021 JFrog Ltd

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
*/

#[cfg(test)]
mod tests {
    extern crate derive_builder;
    extern crate anyhow;
    extern crate pyrsia_client_lib;
    extern crate serde;

    use pyrsia_client_lib::signed::{
        create_key_pair, Attestation, JwsSignatureAlgorithms, SignatureKeyPair, Signed,
    };

    // Create a signed struct in its own module to prevent direct access to its fields. They should
    // be access through the generated getters and setters.
    pub mod foo {
        use pyrsia_client_lib::signed::Signed;
        use signed_struct::signed_struct;

        #[signed_struct]
        pub struct Foo<'a> {
            foo: String,
            bar: u32,
            zot: &'a str,
            zing: Option<u64>
        }
    }
    use foo::*;

    #[test]
    fn test_generated_methods() -> Result<(), anyhow::Error> {
        // Create a signature pair for signing JSON.
        let key_pair: SignatureKeyPair = create_key_pair(JwsSignatureAlgorithms::RS512)?;

        // Values to use for populating our first signed struct.
        let foo_value: String = String::from("abc");
        let foo_value_clone = foo_value.clone();
        let bar_value: u32 = 234;
        let zot_value: &str = "qwerty";

        //Test methods of Foo generated by [signed_struct]
        //let mut foo: Foo = Foo::new(foo_value, bar_value, zot_value);
        let mut foo: Foo = FooBuilder::default().foo(foo_value).bar(bar_value).zing(3894u64 ).build()?;

        foo.sign_json(
            JwsSignatureAlgorithms::RS512,
            &key_pair.private_key,
            &key_pair.public_key,
        )?;
        // Since we have not modified the contents of the struct, its signature should verify
        // successfully.
        foo.verify_signature()?;
        assert_eq!(foo_value_clone, *foo.foo()); // The * is needed because the getters add a & to the type.
        assert_eq!(bar_value, *foo.bar());
        assert_eq!(zot_value, *foo.zot());

        // after signing, there should be json.
        assert!(foo.json().is_some());

        // Now we are going to exercise the generated setters, which should have the side effect of
        // clearing the signed JSON.
        let foo_value: String = String::from("xyz");
        let foo_value_clone = foo_value.clone();
        let bar_value: u32 = 736;
        let zot_value: &str = "asdf";

        foo.set_foo(foo_value);
        foo.set_bar(bar_value);
        foo.set_zot(zot_value);

        assert_eq!(foo_value_clone, *foo.foo()); // The * is needed because the getters add a & to the type.
        assert_eq!(bar_value, *foo.bar());
        assert_eq!(zot_value, *foo.zot());

        // after previous set calls there should be no JSON.
        assert!(foo.json().is_none());

        // Create new JSON by signing.
        foo.sign_json(
            JwsSignatureAlgorithms::RS512,
            &key_pair.private_key,
            &key_pair.public_key,
        )?;

        // after signing, there should be json.
        assert!(foo.json().is_some());

        let json: &str = &foo.json().unwrap();
        println!("JSON: {}", json);

        // Create a copy of the first instance of `Foo` from its signed JSON
        let foo2: Foo = Foo::from_json_string(json)?;

        // after being created from json the signature should be valid and we can examine
        // information about the signature.
        let attestations: Vec<Attestation> = foo2.verify_signature()?;

        // We just signed it once.
        assert_eq!(attestations.len(), 1);

        // Check that the signature information is as expected.
        let attestation = &attestations[0];
        assert!(attestation.signature_is_valid());
        assert!(attestation.signature_algorithm().is_some());
        assert_eq!(
            &JwsSignatureAlgorithms::RS512,
            &attestation.signature_algorithm().unwrap()
        );
        assert!(attestation.expiration_time().is_none());
        assert!(attestation.timestamp().is_some());
        assert!(attestation.public_key().is_some());
        assert_eq!(
            key_pair.public_key,
            attestation.public_key().clone().unwrap()
        );
        Ok(())
    }
}
