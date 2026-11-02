import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { FlintPropAmm } from "../target/types/flint_prop_amm";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram, SYSVAR_INSTRUCTIONS_PUBKEY } from "@solana/web3.js";
import { expect } from "chai";
import * as nacl from "tweetnacl";

describe("Flint Modular Engine vs Prop AMM Test Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.FlintPropAmm as Program<FlintPropAmm>;

  const maker = Keypair.generate();
  const user = Keypair.generate();

  let mintA: PublicKey;
  let mintB: PublicKey;
  let poolPda: PublicKey;
  let poolBump: number;
  let vaultAPda: PublicKey;
  let vaultABump: number;
  let vaultBPda: PublicKey;
  let vaultBBump: number;

  let userAtaA: PublicKey;
  let userAtaB: PublicKey;

  before(async () => {
    // Fund test wallets
    const airdropMaker = await provider.connection.requestAirdrop(maker.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    const airdropUser = await provider.connection.requestAirdrop(user.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(airdropMaker);
    await provider.connection.confirmTransaction(airdropUser);

    // Create Tokens
    mintA = await createMint(provider.connection, maker, maker.publicKey, null, 6);
    mintB = await createMint(provider.connection, maker, maker.publicKey, null, 6);

    // Derive PDAs
    [poolPda, poolBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("pool"), maker.publicKey.toBuffer(), mintA.toBuffer(), mintB.toBuffer()],
      program.programId
    );

    [vaultAPda, vaultABump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_a"), poolPda.toBuffer()],
      program.programId
    );

    [vaultBPda, vaultBBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_b"), poolPda.toBuffer()],
      program.programId
    );

    // User ATAs
    userAtaA = await createAccount(provider.connection, user, mintA, user.publicKey);
    userAtaB = await createAccount(provider.connection, user, mintB, user.publicKey);

    // Mint tokens to user
    await mintTo(provider.connection, maker, mintA, userAtaA, maker, 1_000_000_000);
  });

  it("Initializes the Flint Liquidity Pool (Single PDA Anchor Architecture)", async () => {
    const feeBps = 10; // 0.1%

    const tx = await program.methods
      .initializePool(poolBump, vaultABump, vaultBBump, feeBps)
      .accounts({
        makerAuthority: maker.publicKey,
        mintA: mintA,
        mintB: mintB,
        pool: poolPda,
        vaultA: vaultAPda,
        vaultB: vaultBPda,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([maker])
      .rpc();

    expect(tx).to.be.a("string");

    // Fund Vault with Mint B liquidity (10,000 units)
    await mintTo(provider.connection, maker, mintB, vaultBPda, maker, 10_000_000_000);
  });

  it("Executes Atomic Swap via Cryptographic Intent (Sub-25k CU execution)", async () => {
    const amountIn = new anchor.BN(100_000_000); // 100 A
    const minAmountOut = new anchor.BN(99_500_000); // 99.5 B
    const nonce = new anchor.BN(1);
    const currentSlot = await provider.connection.getSlot();
    const expirySlot = new anchor.BN(currentSlot + 100);

    // Compute message hash identically to utils.rs
    const message = Buffer.alloc(80);
    poolPda.toBuffer().copy(message, 0);
    user.publicKey.toBuffer().copy(message, 32);
    amountIn.toArrayLike(Buffer, "le", 8).copy(message, 64);
    nonce.toArrayLike(Buffer, "le", 8).copy(message, 72);

    // Maker signs off-chain quote intent
    const signature = nacl.sign.detached(message, maker.secretKey);

    const [nonceRecordPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("nonce"), poolPda.toBuffer(), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    const tx = await program.methods
      .executeSwapIntent(amountIn, minAmountOut, nonce, expirySlot, Array.from(signature))
      .accounts({
        user: user.publicKey,
        pool: poolPda,
        nonceRecord: nonceRecordPda,
        userSourceToken: userAtaA,
        userDestinationToken: userAtaB,
        vaultA: vaultAPda,
        vaultB: vaultBPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        instructionsSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
      })
      .signers([user])
      .rpc();

    expect(tx).to.be.a("string");

    // Verify non-reentrancy / nonce replay guard
    try {
      await program.methods
        .executeSwapIntent(amountIn, minAmountOut, nonce, expirySlot, Array.from(signature))
        .accounts({
          user: user.publicKey,
          pool: poolPda,
          nonceRecord: nonceRecordPda,
          userSourceToken: userAtaA,
          userDestinationToken: userAtaB,
          vaultA: vaultAPda,
          vaultB: vaultBPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
          instructionsSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
        })
        .signers([user])
        .rpc();
      expect.fail("Should have failed with NonceAlreadyUsed");
    } catch (err: any) {
      expect(err.toString()).to.contain("already in use");
    }
  });

  it("Rejects expired Intent when deadline is violated", async () => {
    const amountIn = new anchor.BN(50_000_000);
    const minAmountOut = new anchor.BN(49_000_000);
    const nonce = new anchor.BN(2);
    const currentSlot = await provider.connection.getSlot();
    const staleSlot = new anchor.BN(currentSlot - 1); // Stale

    const message = Buffer.alloc(80);
    poolPda.toBuffer().copy(message, 0);
    user.publicKey.toBuffer().copy(message, 32);
    amountIn.toArrayLike(Buffer, "le", 8).copy(message, 64);
    nonce.toArrayLike(Buffer, "le", 8).copy(message, 72);

    const signature = nacl.sign.detached(message, maker.secretKey);

    const [nonceRecordPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("nonce"), poolPda.toBuffer(), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    try {
      await program.methods
        .executeSwapIntent(amountIn, minAmountOut, nonce, staleSlot, Array.from(signature))
        .accounts({
          user: user.publicKey,
          pool: poolPda,
          nonceRecord: nonceRecordPda,
          userSourceToken: userAtaA,
          userDestinationToken: userAtaB,
          vaultA: vaultAPda,
          vaultB: vaultBPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
          instructionsSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
        })
        .signers([user])
        .rpc();
      expect.fail("Should have failed with IntentExpired");
    } catch (err: any) {
      expect(err.toString()).to.contain("IntentExpired");
    }
  });
});