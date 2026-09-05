# Set network to Devnet
solana config set --url https://api.devnet.solana.com

# Fund deployer account
solana airdrop 2

# Deploy Flint Program
anchor deploy --provider.cluster devnet

# Verify Program Deployment
solana program show FLiNTqX1P4o2k9R8v7N2jZcE8VfBqH9m1bY4sA3gM9e