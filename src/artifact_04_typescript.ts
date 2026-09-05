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
import { Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import { expect } from "chai";

describe("Flint Prop AMM: Automated Test Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.FlintPropAmm as Program<FlintPropAmm>;

  let baseMint: PublicKey;
  let quoteMint: PublicKey;
  let poolConfigPda: PublicKey;
  let poolAuthPda: PublicKey;
  let baseVaultPda: PublicKey;
  let quoteVaultPda: PublicKey;

  const authority = Keypair.generate();
  const trader = Keypair.generate();

  let traderBaseAccount: PublicKey;
  let traderQuoteAccount: PublicKey;

  const BASE_DECIMALS = 9; // SOL-like
  const QUOTE_DECIMALS = 6; // USDC-like

  before(async () => {
    // Fund test wallets
    const sig1 = await provider.connection.requestAirdrop(authority.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    const sig2 = await provider.connection.requestAirdrop(trader.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(sig1);
    await provider.connection.confirmTransaction(sig2);

    // Create Mints
    baseMint = await createMint(provider.connection, authority, authority.publicKey, null, BASE_DECIMALS);
    quoteMint = await createMint(provider.connection, authority, authority.publicKey, null, QUOTE_DECIMALS);

    // Derive PDAs
    [poolConfigPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("flint_pool"), baseMint.toBuffer(), quoteMint.toBuffer()],
      program.programId
    );
    [poolAuthPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_auth"), poolConfigPda.toBuffer()],
      program.programId
    );
    [baseVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("base_vault"), poolConfigPda.toBuffer()],
      program.programId
    );
    [quoteVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("quote_vault"), poolConfigPda.toBuffer()],
      program.programId
    );

    // Setup Trader Accounts
    traderBaseAccount = await createAccount(provider.connection, trader, baseMint, trader.publicKey);
    traderQuoteAccount = await createAccount(provider.connection, trader, quoteMint, trader.publicKey);

    // Seed Trader Balances
    await mintTo(provider.connection, authority, baseMint, traderBaseAccount, authority, 1_000 * 10 ** BASE_DECIMALS);
    await mintTo(provider.connection, authority, quoteMint, traderQuoteAccount, authority, 100_000 * 10 ** QUOTE_DECIMALS);
  });

  it("TEST 1: Successfully initializes Flint pool with non-zero inventory targets", async () => {
    const baseFeeBps = 20; // 0.20%
    const skewSensitivity = 500; // 5%
    const targetBase = new anchor.BN(100 * 10 ** BASE_DECIMALS);
    const targetQuote = new anchor.BN(15_000 * 10 ** QUOTE_DECIMALS);

    await program.methods
      .initializePool(baseFeeBps, skewSensitivity, targetBase, targetQuote)
      .accounts({
        poolConfig: poolConfigPda,
        poolAuthority: poolAuthPda,
        baseMint,
        quoteMint,
        baseVault: baseVaultPda,
        quoteVault: quoteVaultPda,
        payer: authority.publicKey,
        authority: authority.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([authority])
      .rpc();

    const pool = await program.account.poolConfig.fetch(poolConfigPda);
    expect(pool.baseMint.toBase58()).to.equal(baseMint.toBase58());
    expect(pool.baseFeeBps).to.equal(baseFeeBps);
    expect(pool.isLocked).to.be.false;

    // Seed pool vaults to simulate established inventory
    await mintTo(provider.connection, authority, baseMint, baseVaultPda, authority, 100 * 10 ** BASE_DECIMALS);
    await mintTo(provider.connection, authority, quoteMint, quoteVaultPda, authority, 15_000 * 10 ** QUOTE_DECIMALS);
  });

  it("TEST 2: Executes normal Base -> Quote swap under fresh Oracle feed", async () => {
    const amountIn = new anchor.BN(1 * 10 ** BASE_DECIMALS); // 1 SOL
    const minAmountOut = new anchor.BN(140 * 10 ** QUOTE_DECIMALS); // Expect ~150 USDC minus fee
    const currentSlot = await provider.connection.getSlot();
    const oraclePrice = new anchor.BN(150_000_000); // 150.000000 USDC/SOL

    const tx = await program.methods
      .swap(amountIn, minAmountOut, oraclePrice, new anchor.BN(currentSlot))
      .accounts({
        poolConfig: poolConfigPda,
        poolAuthority: poolAuthPda,
        baseMint,
        quoteMint,
        baseVault: baseVaultPda,
        quoteVault: quoteVaultPda,
        userTokenSource: traderBaseAccount,
        userTokenDestination: traderQuoteAccount,
        user: trader.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([trader])
      .rpc();

    expect(tx).to.be.a("string");
    const quoteBal = await getAccount(provider.connection, traderQuoteAccount);
    expect(Number(quoteBal.amount)).to.be.greaterThan(100_000 * 10 ** QUOTE_DECIMALS);
  });

  it("TEST 3: Rejects swap when Oracle feed violates staleness threshold (> 25 slots)", async () => {
    const amountIn = new anchor.BN(1 * 10 ** BASE_DECIMALS);
    const minAmountOut = new anchor.BN(100 * 10 ** QUOTE_DECIMALS);
    const staleSlot = new anchor.BN((await provider.connection.getSlot()) - 30);
    const oraclePrice = new anchor.BN(150_000_000);

    try {
      await program.methods
        .swap(amountIn, minAmountOut, oraclePrice, staleSlot)
        .accounts({
          poolConfig: poolConfigPda,
          poolAuthority: poolAuthPda,
          baseMint,
          quoteMint,
          baseVault: baseVaultPda,
          quoteVault: quoteVaultPda,
          userTokenSource: traderBaseAccount,
          userTokenDestination: traderQuoteAccount,
          user: trader.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([trader])
        .rpc();
      expect.fail("Transaction should have failed due to stale oracle feed");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("StaleOracleFeed");
    }
  });

  it("TEST 4: Rejects swap when user slippage exceeds calculated return", async () => {
    const amountIn = new anchor.BN(1 * 10 ** BASE_DECIMALS);
    const excessiveMinOut = new anchor.BN(500 * 10 ** QUOTE_DECIMALS); // Demanding 500 USDC for 150 priced asset
    const currentSlot = await provider.connection.getSlot();
    const oraclePrice = new anchor.BN(150_000_000);

    try {
      await program.methods
        .swap(amountIn, excessiveMinOut, oraclePrice, new anchor.BN(currentSlot))
        .accounts({
          poolConfig: poolConfigPda,
          poolAuthority: poolAuthPda,
          baseMint,
          quoteMint,
          baseVault: baseVaultPda,
          quoteVault: quoteVaultPda,
          userTokenSource: traderBaseAccount,
          userTokenDestination: traderQuoteAccount,
          user: trader.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([trader])
        .rpc();
      expect.fail("Transaction should have failed with SlippageExceeded");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("SlippageExceeded");
    }
  });

  it("TEST 5: Restricts keeper updates to authorized authority", async () => {
    const attacker = Keypair.generate();
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(attacker.publicKey, 10 ** 9)
    );

    try {
      await program.methods
        .keeperUpdateTargets(new anchor.BN(50), new anchor.BN(50), 10)
        .accounts({
          poolConfig: poolConfigPda,
          authority: attacker.publicKey,
        })
        .signers([attacker])
        .rpc();
      expect.fail("Unauthorized keeper must fail");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("UnauthorizedKeeper");
    }
  });
});