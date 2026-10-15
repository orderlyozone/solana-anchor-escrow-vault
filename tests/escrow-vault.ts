import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

describe("solana-anchor-escrow-vault", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.EscrowVault as any;

  const initializer = Keypair.generate();
  const recipient = Keypair.generate();
  const arbitrator = Keypair.generate();

  let mint: PublicKey;
  let initializerTokenAccount: PublicKey;
  let recipientTokenAccount: PublicKey;
  let escrowState: PublicKey;
  let vaultTokenAccount: PublicKey;

  const escrowId = new anchor.BN(Date.now());
  const totalDeposit = new anchor.BN(10_000_000); // 10 tokens (with 6 decimals)
  const milestoneCount = 4;

  before(async () => {
    // Air drop SOL to participants
    const airdropSig1 = await provider.connection.requestAirdrop(
      initializer.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropSig1);

    const airdropSig2 = await provider.connection.requestAirdrop(
      arbitrator.publicKey,
      anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropSig2);

    // Create test mint
    mint = await createMint(
      provider.connection,
      initializer,
      initializer.publicKey,
      null,
      6
    );

    // Create Token Accounts
    initializerTokenAccount = await createAccount(
      provider.connection,
      initializer,
      mint,
      initializer.publicKey
    );

    recipientTokenAccount = await createAccount(
      provider.connection,
      initializer,
      mint,
      recipient.publicKey
    );

    // Mint test tokens to initializer
    await mintTo(
      provider.connection,
      initializer,
      mint,
      initializerTokenAccount,
      initializer,
      100_000_000
    );

    // Derive PDA addresses
    [escrowState] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("escrow"),
        initializer.publicKey.toBuffer(),
        escrowId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    [vaultTokenAccount] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), escrowState.toBuffer()],
      program.programId
    );
  });

  it("Initializes multi-milestone escrow vault", async () => {
    const unlockTimestamps = [
      new anchor.BN(0),
      new anchor.BN(Math.floor(Date.now() / 1000) + 3600),
      new anchor.BN(Math.floor(Date.now() / 1000) + 7200),
      new anchor.BN(Math.floor(Date.now() / 1000) + 10800),
    ];

    await program.methods
      .initializeEscrow(
        escrowId,
        totalDeposit,
        milestoneCount,
        unlockTimestamps
      )
      .accounts({
        initializer: initializer.publicKey,
        recipient: recipient.publicKey,
        arbitrator: arbitrator.publicKey,
        mint: mint,
        initializerTokenAccount: initializerTokenAccount,
        vaultTokenAccount: vaultTokenAccount,
        escrowState: escrowState,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([initializer])
      .rpc();

    const vaultInfo = await getAccount(provider.connection, vaultTokenAccount);
    expect(Number(vaultInfo.amount)).to.equal(10_000_000);

    const state = await program.account.escrowState.fetch(escrowState);
    expect(state.totalAmount.toNumber()).to.equal(10_000_000);
    expect(state.milestonesCompleted).to.equal(0);
    expect(state.isActive).to.be.true;
  });

  it("Releases milestone 1 to recipient", async () => {
    const recipientBalanceBefore = (
      await getAccount(provider.connection, recipientTokenAccount)
    ).amount;

    await program.methods
      .releaseMilestone()
      .accounts({
        authority: initializer.publicKey,
        escrowState: escrowState,
        vaultTokenAccount: vaultTokenAccount,
        recipientTokenAccount: recipientTokenAccount,
        recipient: recipient.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([initializer])
      .rpc();

    const recipientBalanceAfter = (
      await getAccount(provider.connection, recipientTokenAccount)
    ).amount;
    const expectedReleased = 2_500_000n; // 10M / 4 milestones
    expect(recipientBalanceAfter - recipientBalanceBefore).to.equal(expectedReleased);

    const state = await program.account.escrowState.fetch(escrowState);
    expect(state.milestonesCompleted).to.equal(1);
    expect(state.amountRemaining.toNumber()).to.equal(7_500_000);
  });
});
