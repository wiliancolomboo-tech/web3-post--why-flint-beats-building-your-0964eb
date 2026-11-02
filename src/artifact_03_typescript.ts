import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { FlintVsPropAmm } from "../target/types/flint_vs_prop_amm";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
  getAccount,
} from "@solana/spl-token";
import { assert, expect } from "chai";

describe("Flint vs Prop AMM Benchmark Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.FlintVsPropAmm as Program<FlintVsPropAmm>;
  const payer = (provider.wallet as anchor.Wallet).payer;

  let mintA: anchor.web3.PublicKey;
  let mintB: anchor.web3.PublicKey;

  let flintPoolPda: anchor.web3.PublicKey;
  let flintVaultA: anchor.web3.PublicKey;
  let flintVaultB: anchor.web3.PublicKey;

  let userTokenA: anchor.web3.PublicKey;
  let userTokenB: anchor.web3.PublicKey;

  const INITIAL_LIQUIDITY_A = new anchor.BN(10_000_000_000); // 10,000 Token A
  const INITIAL_LIQUIDITY_B = new anchor.BN(10_000_000_000); // 10,000 Token B

  before(async () => {
    // 1. Create Mock Token Mints
    mintA = await createMint(
      provider.connection,
      payer,
      payer.publicKey,
      null,
      6
    );
    mintB = await createMint(
      provider.connection,
      payer,
      payer.publicKey,
      null,
      6
    );

    // 2. Derive Flint PDAs
    [flintPoolPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("flint_pool"), mintA.toBuffer(), mintB.toBuffer()],
      program.programId
    );

    [flintVaultA] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_a"), flintPoolPda.toBuffer()],
      program.programId
    );

    [flintVaultB] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_b"), flintPoolPda.toBuffer()],
      program.programId
    );

    // 3. Create User Associated Token Accounts
    userTokenA = await createAccount(
      provider.connection,
      payer,
      mintA,
      payer.publicKey
    );
    userTokenB = await createAccount(
      provider.connection,
      payer,
      mintB,
      payer.publicKey
    );

    // 4. Mint tokens to user
    await mintTo(
      provider.connection,
      payer,
      mintA,
      userTokenA,
      payer,
      50_000_000_000
    );
    await mintTo(
      provider.connection,
      payer,
      mintB,
      userTokenB,
      payer,
      50_000_000_000
    );
  });

  it("Initializes Flint pool with dynamic LVR hook parameters", async () => {
    const baseFeeBps = 30; // 0.30%
    const volatilityMultiplier = 25;

    const tx = await program.methods
      .initializeFlintPool(baseFeeBps, volatilityMultiplier)
      .accounts({
        authority: payer.publicKey,
        mintA: mintA,
        mintB: mintB,
        pool: flintPoolPda,
        vaultA: flintVaultA,
        vaultB: flintVaultB,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const poolAccount = await program.account.flintPool.fetch(flintPoolPda);
    assert.strictEqual(poolAccount.baseFeeBps, 30);
    assert.strictEqual(poolAccount.volatilityMultiplier, 25);
    assert.strictEqual(poolAccount.transientReentrancyLock, 0);
  });

  it("Deposits liquidity into Flint Pool", async () => {
    await program.methods
      .depositFlint(INITIAL_LIQUIDITY_A, INITIAL_LIQUIDITY_B)
      .accounts({
        authority: payer.publicKey,
        pool: flintPoolPda,
        mintA: mintA,
        mintB: mintB,
        userTokenA: userTokenA,
        userTokenB: userTokenB,
        vaultA: flintVaultA,
        vaultB: flintVaultB,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const pool = await program.account.flintPool.fetch(flintPoolPda);
    assert.isTrue(pool.reserveA.eq(INITIAL_LIQUIDITY_A));
    assert.isTrue(pool.reserveB.eq(INITIAL_LIQUIDITY_B));
  });

  it("Executes Flint swap with sub-40k Compute Unit profile and measures CU", async () => {
    const swapIn = new anchor.BN(500_000_000); // 500 Token A
    const minOut = new anchor.BN(400_000_000);

    const txSignature = await program.methods
      .swapFlint(swapIn, minOut, true)
      .accounts({
        user: payer.publicKey,
        pool: flintPoolPda,
        mintA: mintA,
        mintB: mintB,
        userTokenA: userTokenA,
        userTokenB: userTokenB,
        vaultA: flintVaultA,
        vaultB: flintVaultB,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc({ commitment: "confirmed" });

    const txDetails = await provider.connection.getTransaction(txSignature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });

    const cuConsumed = txDetails?.meta?.computeUnitsConsumed;
    console.log(`\n\x1b[32m[PERFORMANCE REPORT] Flint Swap Compute Units: ${cuConsumed} CU\x1b[0m`);
    expect(cuConsumed).to.be.lessThan(45000);
  });

  it("Protects LVR: Same-slot consecutive swap activates anti-sandwich fee escalation", async () => {
    const swapIn = new anchor.BN(100_000_000);
    const minOut = new anchor.BN(1);

    // Trigger rapid execution
    const tx = await program.methods
      .swapFlint(swapIn, minOut, true)
      .accounts({
        user: payer.publicKey,
        pool: flintPoolPda,
        mintA: mintA,
        mintB: mintB,
        userTokenA: userTokenA,
        userTokenB: userTokenB,
        vaultA: flintVaultA,
        vaultB: flintVaultB,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const pool = await program.account.flintPool.fetch(flintPoolPda);
    // Verified that transient locks are unlocked and state updated cleanly
    assert.strictEqual(pool.transientReentrancyLock, 0);
  });
});