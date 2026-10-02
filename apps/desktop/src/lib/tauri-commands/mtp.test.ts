import { describe, expect, it } from 'vitest'
import { getMtpDeviceDisplayName } from './mtp'

describe('getMtpDeviceDisplayName', () => {
  it('uses product name when available', () => {
    expect(
      getMtpDeviceDisplayName({
        id: 'mtp-336592896',
        locationId: 336592896,
        vendorId: 0x18d1,
        productId: 0x4ee1,
        manufacturer: 'Google',
        product: 'Pixel 8',
      }),
    ).toBe('Pixel 8')
  })

  it('uses manufacturer name when product is missing', () => {
    expect(
      getMtpDeviceDisplayName({
        id: 'mtp-336592897',
        locationId: 336592897,
        vendorId: 0x04e8,
        productId: 0x6860,
        manufacturer: 'Samsung',
      }),
    ).toBe('Samsung device')
  })

  it('uses vendor:product format as fallback', () => {
    expect(
      getMtpDeviceDisplayName({ id: 'mtp-336592898', locationId: 336592898, vendorId: 0x1234, productId: 0x5678 }),
    ).toBe('MTP device (1234:5678)')
  })
})
