import XCTest
@testable import AgentRelayiPad

final class VoiceCallPolicyTests: XCTestCase {
    private func allows(region: String?, storefront: String?) -> Bool {
        VoiceCallPolicy.allowsCallKit(regionIdentifier: region, storefrontCountryCode: storefront)
    }

    func testCallKitOutsideChinaMainland() {
        XCTAssertTrue(allows(region: "US", storefront: "USA"))
        XCTAssertTrue(allows(region: "GB", storefront: "GBR"))
        XCTAssertTrue(allows(region: nil, storefront: "DEU"))
    }

    func testChinaMainlandStorefrontTurnsCallKitOff() {
        XCTAssertFalse(allows(region: "US", storefront: "CHN"))
        XCTAssertFalse(allows(region: nil, storefront: "CHN"))
        XCTAssertFalse(allows(region: "US", storefront: "chn"))
        XCTAssertFalse(allows(region: "US", storefront: " CHN "))
    }

    func testChinaMainlandRegionTurnsCallKitOff() {
        XCTAssertFalse(allows(region: "CN", storefront: "USA"))
        XCTAssertFalse(allows(region: "cn", storefront: "USA"))
        XCTAssertFalse(allows(region: "CN", storefront: "CHN"))
    }

    func testUnknownStorefrontLeavesTheDecisionToTheRegion() {
        XCTAssertTrue(allows(region: "US", storefront: nil))
        XCTAssertTrue(allows(region: "US", storefront: ""))
        XCTAssertTrue(allows(region: nil, storefront: nil))
        XCTAssertFalse(allows(region: "CN", storefront: nil))
        XCTAssertFalse(allows(region: "CN", storefront: " "))
    }

    func testHongKongMacauAndTaiwanKeepCallKit() {
        XCTAssertTrue(allows(region: "HK", storefront: "HKG"))
        XCTAssertTrue(allows(region: "MO", storefront: "MAC"))
        XCTAssertTrue(allows(region: "TW", storefront: "TWN"))
    }
}
