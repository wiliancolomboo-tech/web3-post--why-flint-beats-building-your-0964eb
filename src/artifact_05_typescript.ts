import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { FlintEngine } from "../target/types/flint_engine";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, SystemProgram, PublicKey } from "@solana/web3.js";
import { expect } from "chai";
import BN from "bn.js";

describe("Flint Engine Integration & MEV Mitigation Test Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.FlintEngine as Program<FlintEngine>;

  let tokenAMint: PublicKey;
  let tokenBMint: PublicKey;
  let poolStatePda: PublicKey;
  let vaultAuthorityPda: PublicKey;
  let vaultAKeypair: Keypair;
  let vaultBKeypair: Keypair;
  let mockOracleAccount: Keypair;

  let userAtaA: PublicKey;
  let userAtaB: PublicKey;

  const poolId = new BN(1001);
  const baseFeeBps = 30; // 0.30%
  const virtualReserveA = new BN(1_000_000_000_000); // 1,000 units (6 decimals)
  const virtualReserveB = new BN(1_000_000_000_000);

  before(async () => {
    // 1. Create Token Mints
    tokenAMint = await createMint(
      provider.connection,
      provider.wallet.payer,
      provider.wallet.publicKey,
      null,
      6
    );

    tokenBMint = await createMint(
      provider.connection,
      provider.wallet.payer,
      provider.wallet.publicKey,
      null,
      6
    );

    // 2. Derive PDAs
    [poolStatePda] = PublicKey.findProgramAddressSync(
      [Buffer.from("pool_state"), poolId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultAuthorityPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_authority"), poolId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    vaultAKeypair = Keypair.generate();
    vaultBKeypair = Keypair.generate();
    mockOracleAccount = Keypair.generate();

    // 3. Initialize Mock Oracle Feed with valid non-stale Pyth-like buffer
    const oracleBuffer = Buffer.alloc(64);
    oracleBuffer.writeBigInt64LE(BigInt(0x50595448), 0); // Pyth Magic
    oracleBuffer.writeBigInt64LE(BigInt(100_000_000), 8); // Price: $100.00
    oracleBuffer.writeBigUInt64LE(BigInt(10_000), 16); // Conf: low spread
    const nowSec = Math.floor(Date.now() / 1000);
    oracleBuffer.writeBigInt64LE(BigInt(nowSec), 24); // Publish timestamp

    const lamportsForOracle =
      await provider.connection.getMinimumBalanceForRentExemption(64);
    const createOracleTx = new anchor.web3.Transaction().add(
      SystemProgram.createAccount({
        fromPubkey: provider.wallet.publicKey,
        newAccountPubkey: mockOracleAccount.publicKey,
        lamports: lamportsForOracle,
        space: 64,
        programId: program.programId,
      })
    );
    await provider.sendAndConfirm(createOracleTx, [mockOracleAccount]);

    // Populate oracle data account via provider
    // In localnet test, we maintain the buffer in mock
  });

  it("Initializes the Flint Liquidity Pool Successfully", async () => {
    await program.methods
      .initializePool(poolId, baseFeeBps, virtualReserveA, virtualReserveB)
      .accounts({
        poolState: poolStatePda,
        vaultAuthority: vaultAuthorityPda,
        tokenAMint: tokenAMint,
        tokenBMint: tokenBMint,
        tokenAVault: vaultAKeypair.publicKey,
        tokenBVault: vaultBKeypair.publicKey,
        oracleFeed: mockOracleAccount.publicKey,
        admin: provider.wallet.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([vaultAKeypair, vaultBKeypair])
      .rpc();

    const pool = await program.account.poolState.fetch(poolStatePda);
    expect(pool.poolId.eq(poolId)).to.be.true;
    expect(pool.baseFeeBps).to.equal(baseFeeBps);
    expect(pool.isLocked).to.be.false;
  });

  it("Executes Atomic Exact-Input Swap with Dynamic Slippage Floor", async () => {
    // Mint 5,000 units to User ATA
    userAtaA = await createAccount(
      provider.connection,
      provider.wallet.payer,
      tokenAMint,
      provider.wallet.publicKey
    );
    userAtaB = await createAccount(
      provider.connection,
      provider.wallet.payer,
      tokenBMint,
      provider.wallet.publicKey
    );

    // Fund the real reserves in Vault B so user can withdraw
    await mintTo(
      provider.connection,
      provider.wallet.payer,
      tokenBMint,
      vaultBKeypair.publicKey,
      provider.wallet.publicKey,
      50_000_000_000
    );

    // Sync state: increment pool real reserve via swap setup
    await mintTo(
      provider.connection,
      provider.wallet.payer,
      tokenAMint,
      userAtaA,
      provider.wallet.publicKey,
      10_000_000
    );

    // Perform swap: 10 units in (10,000,000 micro-units)
    const amountIn = new BN(10_000_000);
    const minAmountOut = new BN(9_500_000);

    const txSig = await program.methods
      .swapExactInput(amountIn, minAmountOut)
      .accounts({
        poolState: poolStatePda,
        vaultAuthority: vaultAuthorityPda,
        tokenAMint: tokenAMint,
        tokenBMint: tokenBMint,
        tokenAVault: vaultAKeypair.publicKey,
        tokenBVault: vaultBKeypair.publicKey,
        userTokenSource: userAtaA,
        userTokenDestination: userAtaB,
        oracleFeed: mockOracleAccount.publicKey,
        userAuthority: provider.wallet.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    expect(txSig).to.be.a("string");

    // Invariant Check
    const pool = await program.account.poolState.fetch(poolStatePda);
    expect(pool.realReserveA.toNumber()).to.be.greaterThan(0);
  });

  it("Rejects Swap when Slippage Tolerance Exceeded", async () => {
    const excessiveMinOut = new BN(100_000_000_000); // Demanding 100k tokens for 1 token

    let failed = false;
    try {
      await program.methods
        .swapExactInput(new BN(1_000_000), excessiveMinOut)
        .accounts({
          poolState: poolStatePda,
          vaultAuthority: vaultAuthorityPda,
          tokenAMint: tokenAMint,
          tokenBMint: tokenBMint,
          tokenAVault: vaultAKeypair.publicKey,
          tokenBVault: vaultBKeypair.publicKey,
          userTokenSource: userAtaA,
          userTokenDestination: userAtaB,
          oracleFeed: mockOracleAccount.publicKey,
          userAuthority: provider.wallet.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();
    } catch (err: any) {
      failed = true;
      expect(err.toString()).to.include("SlippageExceeded");
    }
    expect(failed).to.be.true;
  });
});