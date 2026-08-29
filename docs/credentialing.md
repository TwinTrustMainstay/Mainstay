# Engineer Credentialing System

This document describes the federated credentialing system used in Mainstay to verify and manage engineer identities and qualifications for maintenance operations.

## Overview

The engineer credentialing system provides a decentralized, trustless way to verify that only qualified engineers can sign maintenance records on industrial assets. It uses a federated model where trusted issuers can credential engineers, creating a verifiable chain of trust.

## System Architecture

### Key Components

1. **Trusted Issuers** - Organizations authorized to issue credentials
2. **Engineer Credentials** - Cryptographic proofs of engineer qualifications  
3. **Verification System** - On-chain validation of credential status
4. **Revocation Mechanism** - Ability to invalidate compromised credentials

## Credential Lifecycle

### 1. Issuance
- **Who**: Trusted issuer organizations
- **What**: Engineer addresses with credential hashes
- **Validation**: Issuer must be in trusted issuers list
- **Security**: Zero-hash credentials are rejected

### 2. Verification
- **Who**: Anyone can verify
- **What**: Checks both active status and expiration
- **Result**: Boolean indicating current validity
- **Use Case**: Maintenance contract validation

### 3. Revocation
- **Who**: Original issuing authority only
- **What**: Deactivates credential (sets active=false)
- **Persistence**: Record remains for audit trail
- **Security**: Prevents unauthorized revocation

## Data Structures

### Engineer Record
```rust
pub struct Engineer {
    pub address: Address,           // Engineer's wallet address
    pub credential_hash: BytesN<32>, // Hash of qualifications/certs
    pub issuer: Address,            // Who issued the credential
    pub active: bool,               // Current validity status
    pub issued_at: u64,            // When credential was issued
    pub expires_at: u64,            // When credential expires
}
```

### Credential Hash
- **Purpose**: Cryptographic fingerprint of engineer qualifications
- **Content**: Typically hash of certificates, licenses, training records
- **Security**: Prevents credential tampering and forgery
- **Format**: 32-byte SHA-256 hash

## Trusted Issuer Model

### Issuer Registration
- **Authority**: Contract administrators only
- **Validation**: Address verification and authorization
- **Storage**: Instance storage for global access
- **Listing**: Maintained in trusted issuers vector

### Issuer Responsibilities
- **Vetting**: Verify engineer qualifications before issuing
- **Standards**: Follow consistent credentialing standards
- **Security**: Protect issuer private keys
- **Compliance**: Follow regulatory requirements

### Issuer Benefits
- **Reputation**: Build trusted brand in ecosystem
- **Revenue**: Potential credentialing service fees
- **Network**: Connect with qualified engineers
- **Authority**: Participate in governance

## Security Features

### Zero-Hash Protection
```rust
if credential_hash == BytesN::from_array(&env, &[0u8; 32]) {
    panic_with_error!(&env, ContractError::InvalidCredentialHash);
}
```

### Issuer Authorization
- **Registration**: Admin-only function to add issuers
- **Verification**: Only trusted issuers can credential engineers
- **Removal**: Admin-only function to remove issuers
- **Audit Trail**: All changes emit events

### Expiration Handling
- **Automatic**: Credentials expire based on validity period
- **Verification**: Expired credentials return false
- **Flexibility**: Validity period set per credential
- **Renewal**: New credentials issued after expiration

## API Operations

### For Engineers
```rust
// Check if your credential is valid
verify_engineer(your_address) -> bool

// Get your credential details
get_engineer(your_address) -> Engineer

// Find engineers credentialed by same issuer
get_engineers_by_issuer(issuer_address) -> Vec<Address>
```

### For Issuers
```rust
// Register a new engineer
register_engineer(
    engineer_address,
    credential_hash,
    issuer_address,
    validity_period_seconds
)

// Revoke a credential
revoke_credential(engineer_address)

// Check if you're a trusted issuer
is_trusted_issuer(your_address) -> bool
```

### For Administrators
```rust
// Add a trusted issuer
add_trusted_issuer(admin_address, issuer_address)

// Remove a trusted issuer  
remove_trusted_issuer(admin_address, issuer_address)

// Get all trusted issuers
get_trusted_issuers() -> Vec<Address>
```

## Use Cases

### Maintenance Verification
- **Requirement**: Only verified engineers can submit maintenance
- **Process**: Lifecycle contract calls `verify_engineer()`
- **Result**: Maintenance records are trustworthy
- **Benefit**: Prevents fraudulent maintenance claims

### Engineer Onboarding
- **Process**: Engineers apply to trusted issuers
- **Verification**: Issuers validate qualifications
- **Issuance**: Credentials stored on-chain
- **Outcome**: Engineers can perform maintenance

### Credential Management
- **Tracking**: Monitor credential expiration dates
- **Renewal**: Process new credentials before expiry
- **Revocation**: Handle compromised or invalid credentials
- **Audit**: Maintain complete credential history

## Best Practices

### For Engineers
- **Protect Keys**: Secure your private wallet keys
- **Verify Status**: Check credential validity regularly
- **Plan Renewal**: Renew credentials before expiration
- **Choose Issuers**: Select reputable trusted issuers
- **Document**: Keep offline copies of qualifications

### For Issuers
- **Due Diligence**: Thoroughly verify engineer qualifications
- **Standardization**: Use consistent credentialing processes
- **Security**: Implement strong identity verification
- **Record Keeping**: Maintain offline audit trails
- **Communication**: Clear credential terms and conditions

### For Asset Owners
- **Verification**: Always check engineer credential status
- **Reject Invalid**: Don't accept maintenance from unverified engineers
- **Documentation**: Record engineer addresses used
- **Quality**: Prefer engineers from reputable issuers

## Integration Points

### With Lifecycle Contract
- **Automatic Verification**: Maintenance contract validates engineers
- **Event Emission**: Credential changes emit events
- **Security**: Prevents unauthorized maintenance submissions
- **Audit Trail**: Links credentials to maintenance records

### With Asset Registry
- **Independent**: Separate contract for asset management
- **Cross-Reference**: Engineers work across multiple assets
- **Reputation**: Build maintenance history across assets
- **Flexibility**: Support multiple credentialing systems

## Security Considerations

### Threat Model
- **Impersonation**: Stolen engineer credentials
- **False Issuance**: Fraudulent issuer behavior
- **Expired Credentials**: Using outdated qualifications
- **Centralization**: Too few trusted issuers

### Mitigations
- **Cryptography**: Hash-based credential verification
- **Federation**: Multiple independent trusted issuers
- **Expiration**: Time-limited credential validity
- **Revocation**: Quick response to compromised credentials
- **Transparency**: On-chain public verification

## Technical Implementation

### Storage Keys
- **Engineer Data**: `("ENG", engineer_address)`
- **Trusted Issuers**: `("TRUSTED", issuer_address)`
- **Issuer List**: `("ISS_LIST")`
- **Issuer Engineers**: `("ISS_ENGS", issuer_address)`

### TTL Management
- **Duration**: 518,400 seconds (~6 days)
- **Extension**: Automatic on all write operations
- **Purpose**: Prevent data loss and ensure availability

### Error Handling
- **InvalidCredentialHash**: Zero hash rejection
- **UntrustedIssuer**: Non-authorized credentialing attempt
- **EngineerNotFound**: Query for non-existent engineer
- **CredentialAlreadyRevoked**: Duplicate revocation attempt

## Configuration

### Admin Functions
- **initialize_admin()**: Set first administrator
- **get_admin()**: Retrieve current administrator
- **upgrade()**: Update contract code

### Issuer Management
- **add_trusted_issuer()**: Add new credentialing authority
- **remove_trusted_issuer()**: Remove existing authority
- **is_trusted_issuer()**: Check issuer status
- **get_trusted_issuers()**: List all authorities

## Future Enhancements

### Potential Improvements
1. **Multi-Level Credentials**: Different credential levels (basic, advanced, expert)
2. **Specialization**: Credentials for specific asset types or industries
3. **Reputation System**: Engineer ratings based on maintenance quality
4. **Cross-Chain Verification**: Verify credentials from other blockchain networks
5. **Zero-Knowledge Proofs**: Privacy-enhanced credential verification

---

*This documentation describes the credentialing system as implemented in the engineer registry contract. For implementation details, refer to the source code in contracts/engineer-registry/src/lib.rs.*
