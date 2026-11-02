# 1. Clone repository and navigate to root directory
cd flint-hook-amm

# 2. Install TypeScript QA dependencies
yarn install

# 3. Build the Anchor program binary and generate IDL
anchor build

# 4. Extract the Program Keypair Address and verify against declare_id!
solana address -k target/deploy/flint_hook_amm-keypair.json