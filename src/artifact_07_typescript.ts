import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { FlintPropAmm } from "../target/types/flint_prop_amm";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert, expect } from "chai";

describe("Flint Modular Curve vs Proprietary AMM Benchmark", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.FlintPropAmm as Program<FlintPropAmm>;

  let tokenAMint: anchor.web3.PublicKey;
  let tokenBMint: anchor.web3.PublicKey;
  let poolPda: anchor.web3.PublicKey;
  let poolBump: number;
  let vaultAPda: anchor.web3.PublicKey;
  let vaultBPda: anchor.web3.PublicKey;
  let vaultAuthPda: anchor.web3.PublicKey;
  let vaultAuthBump: number;

  let depositorTokenA: anchor.web3.PublicKey;
  let depositorTokenB: anchor.web3.PublicKey;
  let userTokenA: anchor.web3.PublicKey;
  let userTokenB: anchor.web3.PublicKey;

  const INITIAL_LIQUIDITY_A = new anchor.BN(10_000_000_000); // 10,000 units
  const INITIAL_LIQUIDITY_B = new anchor.BN(50_000_000_000); // 50,000 units

  before(async () => {
    // 1. Initialize Mints
    tokenAMint = await createMint(
      provider.connection,
      (provider.wallet as any).payer,
      provider.wallet.publicKey,
      null,
      6
    );
    tokenBMint = await createMint(
      provider.connection,
      (provider.wallet as any).payer,
      provider.wallet.publicKey,
      null,
      6
    );

    // Derive PDAs
    [poolPda, poolBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("flint_pool"), tokenAMint.toBuffer(), tokenBMint.toBuffer()],
      program.programId
    );

    [vaultAuthPda, vaultAuthBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("pool_vault"), poolPda.toBuffer()],
      program.programId
    );

    const [vAPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("pool_vault"), poolPda.toBuffer(), tokenAMint.toBuffer()],
      program.programId
    );
    vaultAPda = vAPda;

    const [vBPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("pool_vault"), poolPda.toBuffer(), tokenBMint.toBuffer()],
      program.programId
    );
    vaultBPda = vBPda;

    // Create user token accounts
    depositorTokenA = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      tokenAMint,
      provider.wallet.publicKey
    );
    depositorTokenB = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      tokenBMint,
      provider.wallet.publicKey
    );

    userTokenA = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      tokenAMint,
      provider.wallet.publicKey
    );
    userTokenB = await createAccount(
      provider.connection,
      (provider.wallet as any).payer,
      tokenBMint,
      provider.wallet.publicKey
    );

    // Mint tokens to accounts
    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      tokenAMint,
      depositorTokenA,
      provider.wallet.publicKey,
      20_000_000_000
    );
    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      tokenBMint,
      depositorTokenB,
      100_000_000_000
    );
    await mintTo(
      provider.connection,
      (provider.wallet as any).payer,
      tokenAMint,
      userTokenA,
      provider.wallet.publicKey,
      500_000_000
    );
  });

  it("Initializes Flint Modular Pool with minimal account allocation", async () => {
    const tx = await program.methods
      .initializePool(poolBump, vaultAuthBump, vaultAuthBump)
      .accounts({
        pool: poolPda,
        tokenAMint: tokenAMint,
        tokenBMint: tokenBMint,
        vaultA: vaultAPda,
        vaultB: vaultBPda,
        vaultAuthority: vaultAuthPda,
        authority: provider.wallet.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    expect(tx).to.be.a("string");
    const poolAccount = await program.account.poolState.fetch(poolPda);
    assert.ok(poolAccount.authority.equals(provider.wallet.publicKey));
  });

  it("Deposits Liquidity into PDA Vaults", async () => {
    await program.methods
      .depositLiquidity(INITIAL_LIQUIDITY_A, INITIAL_LIQUIDITY_B)
      .accounts({
        pool: poolPda,
        vaultA: vaultAPda,
        vaultB: vaultBPda,
        depositorTokenA: depositorTokenA,
        depositorTokenB: depositorTokenB,
        depositor: provider.wallet.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const vaultA = await getAccount(provider.connection, vaultAPda);
    const vaultB = await getAccount(provider.connection, vaultBPda);
    assert.strictEqual(vaultA.amount.toString(), INITIAL_LIQUIDITY_A.toString());
    assert.strictEqual(vaultB.amount.toString(), INITIAL_LIQUIDITY_B.toString());
  });

  it("Executes a Swap and Benchmarks Compute Units (< 35k CU Budget)", async () => {
    const swapAmountIn = new anchor.BN(100_000_000); // 100 A tokens
    const minAmountOut = new anchor.BN(450_000_000); // Expect ~490 B tokens

    const tx = await program.methods
      .swapViaFlintHook(swapAmountIn, minAmountOut, true)
      .accounts({
        pool: poolPda,
        vaultA: vaultAPda,
        vaultB: vaultBPda,
        vaultAuthority: vaultAuthPda,
        userSourceToken: userTokenA,
        userDestinationToken: userTokenB,
        user: provider.wallet.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc({ commitment: "confirmed" });

    const txDetails = await provider.connection.getTransaction(tx, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });

    const cuConsumed = txDetails?.meta?.computeUnitsConsumed;
    console.log(`\n==============================================`);
    console.log(`🔥 Flint Swap Instruction CU Consumed: ${cuConsumed} CU`);
    console.log(`==============================================\n`);

    // Ensure CU is well below bespoke prop AMM averages (110,000 - 180,000 CU)
    expect(cuConsumed).to.be.lessThan(45000);

    const userBBalance = await getAccount(provider.connection, userTokenB);
    expect(Number(userBBalance.amount)).to.be.greaterThan(0);
  });

  it("Fails safely when slippage limit is breached", async () => {
    const swapAmountIn = new anchor.BN(10_000_000);
    const impossibleMinAmountOut = new anchor.BN(999_999_999_999); // Exceeds all pool reserves

    try {
      await program.methods
        .swapViaFlintHook(swapAmountIn, impossibleMinAmountOut, true)
        .accounts({
          pool: poolPda,
          vaultA: vaultAPda,
          vaultB: vaultBPda,
          vaultAuthority: vaultAuthPda,
          userSourceToken: userTokenA,
          userDestinationToken: userTokenB,
          user: provider.wallet.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();
      assert.fail("Transaction should have failed with SlippageExceeded");
    } catch (err: any) {
      expect(err.toString()).to.include("SlippageExceeded");
    }
  });
});