# 1. Install TypeScript dependencies
yarn install

# 2. Compile Anchor Program
anchor build

# 3. Run the automated QA Test Suite with CU logging
anchor test -- --nocapture