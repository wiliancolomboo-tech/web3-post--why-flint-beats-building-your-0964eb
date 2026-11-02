# 1. Clone repository and navigate to root
git clone https://github.com/flint-protocol/flint-vs-prop-amm.git
cd flint-vs-prop-amm

# 2. Install Node dependencies
yarn install

# 3. Build Anchor program and generate IDL types
anchor build

# 4. Synchronize Program ID (Ensure keys match declared ID)
PROGRAM_ID=$(solana-keygen pubkey target/deploy/flint_vs_prop_amm-keypair.json)
sed -i "s/FLiNT111111111111111111111111111111111111111/$PROGRAM_ID/g" programs/flint-vs-prop-amm/src/lib.rs
sed -i "s/FLiNT111111111111111111111111111111111111111/$PROGRAM_ID/g" Anchor.toml

# 5. Re-run build with synchronized Program ID
anchor build

# 6. Execute comprehensive test suite against Solana localnet
anchor test