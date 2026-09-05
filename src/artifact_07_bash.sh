# 1. Clone project and navigate to program root
git clone https://github.com/flint-amm/flint-prop-amm.git
cd flint-prop-amm

# 2. Configure Solana CLI to Localnet
solana config set --url localhost

# 3. Generate Program Deployer Keypair (if not present)
solana-keygen new --no-bip39-passphrase --force -o ~/.config/solana/id.json

# 4. Sync Anchor Program ID
anchor keys sync