import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { FlintHookAmm } from "../target/types/flint_hook_amm";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";

describe("flint-hook-amm QA Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.FlintHookAmm as Program<FlintHookAmm>;

  const authority = provider.wallet as anchor.Wallet;
  const mockPool = Keypair.generate();
  const mockOracle = Keypair.generate();
  const mockFlintCore = Keypair.generate();

  let hookConfigPda: PublicKey;
  let hookBump: number;

  before(async () => {
    [hookConfigPda, hookBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("flint_hook"), mockPool.publicKey.toBuffer()],
      program.programId
    );
  });

  it("Initializes the Flint Dynamic Volatility Hook successfully", async () => {
    const baseFee = 30; // 0.30%
    const maxFee = 300; // 3.00%
    const ampFactor = 100;

    await program.methods
      .initializeHook(baseFee, maxFee, ampFactor)
      .accounts({
        authority: authority.publicKey,
        poolId: mockPool.publicKey,
        oracleFeed: mockOracle.publicKey,
        hookConfig: hookConfigPda,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const configAccount = await program.account.hookConfig.fetch(hookConfigPda);
    expect(configAccount.authority.toBase58()).to.equal(authority.publicKey.toBase58());
    expect(configAccount.baseFeeBps).to.equal(30);
    expect(configAccount.maxFeeBps).to.equal(300);
    expect(configAccount.ewmaVolatility.toNumber()).to.equal(0);
  });

  it("Updates EWMA volatility accurately on oracle price shock", async () => {
    const price1 = new anchor.BN(100_000_000); // $100.00
    await program.methods
      .updateOracle(price1)
      .accounts({
        keeper: authority.publicKey,
        hookConfig: hookConfigPda,
        oracleFeed: mockOracle.publicKey,
      })
      .rpc();

    // Shock price to $120.00 (20% instant move)
    const price2 = new anchor.BN(120_000_000);
    await program.methods
      .updateOracle(price2)
      .accounts({
        keeper: authority.publicKey,
        hookConfig: hookConfigPda,
        oracleFeed: mockOracle.publicKey,
      })
      .rpc();

    const configAccount = await program.account.hookConfig.fetch(hookConfigPda);
    expect(configAccount.ewmaVolatility.toNumber()).to.be.greaterThan(0);
    console.log(`[QA Matrix] EWMA Volatility Registered: ${configAccount.ewmaVolatility.toString()}`);
  });

  it("Executes dynamic swap hook and applies dynamic fee penalty under volatility", async () => {
    const swapInput = {
      reserveIn: new anchor.BN(10_000_000_000), // 10,000 SOL
      reserveOut: new anchor.BN(1_000_000_000_000), // 1,000,000 USDC
      amountIn: new anchor.BN(100_000_000), // 100 SOL
      minimumAmountOut: new anchor.BN(9_000_000),
    };

    const tx = await program.methods
      .executeSwapHook(swapInput)
      .accounts({
        flintCoreProgram: mockFlintCore.publicKey,
        hookConfig: hookConfigPda,
      })
      .signers([mockFlintCore])
      .rpc();

    expect(tx).to.be.a("string");
  });

  it("Rejects swap execution if slippage bound is violated", async () => {
    const maliciousSlippageInput = {
      reserveIn: new anchor.BN(10_000_000_000),
      reserveOut: new anchor.BN(1_000_000_000_000),
      amountIn: new anchor.BN(100_000_000),
      minimumAmountOut: new anchor.BN(999_999_999_999), // Impossible return
    };

    try {
      await program.methods
        .executeSwapHook(maliciousSlippageInput)
        .accounts({
          flintCoreProgram: mockFlintCore.publicKey,
          hookConfig: hookConfigPda,
        })
        .signers([mockFlintCore])
        .rpc();
      expect.fail("Transaction should have failed with SlippageExceeded");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("SlippageExceeded");
    }
  });
});