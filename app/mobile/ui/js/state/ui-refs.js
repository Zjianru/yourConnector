// 文件职责：
// 1. 统一维护页面 DOM 引用映射，提供 UI 节点单一来源。
// 2. 隔离 document 查询逻辑，避免状态层承担视图绑定职责。
// 3. 不包含业务状态或流程行为，仅返回静态节点引用集合。

/**
 * 构建页面 UI 节点引用集合。
 * @returns {Record<string, HTMLElement|null>} 页面节点映射对象。
 */
export function createUiRefs() {
  return {
    // 基础页面。
    opsView: document.getElementById("opsView"),
    debugView: document.getElementById("debugView"),
    tabOps: document.getElementById("tabOps"),
    tabDebug: document.getElementById("tabDebug"),

    // 顶部操作。
    connectBtnTop: document.getElementById("connectBtnTop"),
    disconnectBtnTop: document.getElementById("disconnectBtnTop"),
    replaceHostBtnTop: document.getElementById("replaceHostBtnTop"),

    // 配对与总览。
    hostSetupCard: document.getElementById("hostSetupCard"),
    hostOverviewWrap: document.getElementById("hostOverviewWrap"),
    importPairLinkBtn: document.getElementById("importPairLinkBtn"),
    openManualPairBtn: document.getElementById("openManualPairBtn"),
    openDebugFromSetupBtn: document.getElementById("openDebugFromSetupBtn"),
    hostBannerTrack: document.getElementById("hostBannerTrack"),
    hostBannerDots: document.getElementById("hostBannerDots"),
    toolsGroupedList: document.getElementById("toolsGroupedList"),

    // 调试页。
    debugStatus: document.getElementById("debugStatus"),
    debugEvents: document.getElementById("debugEvents"),
    debugHostSelect: document.getElementById("debugHostSelect"),
    debugIdentity: document.getElementById("debugIdentity"),
    connectBtnDebug: document.getElementById("connectBtnDebug"),
    disconnectBtnDebug: document.getElementById("disconnectBtnDebug"),
    rebindControllerBtn: document.getElementById("rebindControllerBtn"),
    messageInput: document.getElementById("messageInput"),
    sendBtn: document.getElementById("sendBtn"),
    copyOpLogsBtn: document.getElementById("copyOpLogsBtn"),
    clearLogsBtn: document.getElementById("clearLogsBtn"),
    logBox: document.getElementById("logBox"),

    // 工具详情。
    toolModal: document.getElementById("toolModal"),
    toolModalTitle: document.getElementById("toolModalTitle"),
    toolModalClose: document.getElementById("toolModalClose"),
    toolSummaryTitle: document.getElementById("toolSummaryTitle"),
    summaryStatusDots: document.getElementById("summaryStatusDots"),
    summaryRows: document.getElementById("summaryRows"),
    toolDetailSectionTitle: document.getElementById("toolDetailSectionTitle"),
    detailRows: document.getElementById("detailRows"),
    detailTip: document.getElementById("detailTip"),
    toggleDetailsBtn: document.getElementById("toggleDetailsBtn"),
    usagePanel: document.getElementById("usagePanel"),
    usagePanelTitle: document.getElementById("usagePanelTitle"),
    usageRows: document.getElementById("usageRows"),

    // 添加工具。
    addToolModal: document.getElementById("addToolModal"),
    addToolModalClose: document.getElementById("addToolModalClose"),
    candidateList: document.getElementById("candidateList"),
    goDebugFromAddTool: document.getElementById("goDebugFromAddTool"),

    // 配对流程。
    pairFlowModal: document.getElementById("pairFlowModal"),
    pairFlowTitle: document.getElementById("pairFlowTitle"),
    pairFlowClose: document.getElementById("pairFlowClose"),
    pairFlowStepImport: document.getElementById("pairFlowStepImport"),
    pairFlowStepPaste: document.getElementById("pairFlowStepPaste"),
    pairFlowStepScan: document.getElementById("pairFlowStepScan"),
    pairFlowStepManual: document.getElementById("pairFlowStepManual"),
    pairOpenScanBtn: document.getElementById("pairOpenScanBtn"),
    pairOpenPasteBtn: document.getElementById("pairOpenPasteBtn"),
    pairPasteSubmitBtn: document.getElementById("pairPasteSubmitBtn"),
    pairPasteBackBtn: document.getElementById("pairPasteBackBtn"),
    pairScanVideo: document.getElementById("pairScanVideo"),
    pairScanStatus: document.getElementById("pairScanStatus"),
    pairScanFileInput: document.getElementById("pairScanFileInput"),
    pairScanGalleryBtn: document.getElementById("pairScanGalleryBtn"),
    pairScanBackBtn: document.getElementById("pairScanBackBtn"),
    hostRelayInput: document.getElementById("hostRelayInput"),
    hostSystemIdInput: document.getElementById("hostSystemIdInput"),
    hostPairTicketInput: document.getElementById("hostPairTicketInput"),
    hostNameInput: document.getElementById("hostNameInput"),
    pairManualSubmitBtn: document.getElementById("pairManualSubmitBtn"),
    pairManualBackBtn: document.getElementById("pairManualBackBtn"),
    pairLinkInput: document.getElementById("pairLinkInput"),

    // 配对失败弹窗。
    pairFailureModal: document.getElementById("pairFailureModal"),
    pairFailureClose: document.getElementById("pairFailureClose"),
    pairFailureReason: document.getElementById("pairFailureReason"),
    pairFailureSuggestion: document.getElementById("pairFailureSuggestion"),
    pairFailurePrimaryBtn: document.getElementById("pairFailurePrimaryBtn"),
    pairFailureSecondaryBtn: document.getElementById("pairFailureSecondaryBtn"),

    // 宿主管理。
    hostManageModal: document.getElementById("hostManageModal"),
    hostManageClose: document.getElementById("hostManageClose"),
    hostManageList: document.getElementById("hostManageList"),
    pendingDeleteList: document.getElementById("pendingDeleteList"),
    hostManageAddBtn: document.getElementById("hostManageAddBtn"),
    hostManageDebugBtn: document.getElementById("hostManageDebugBtn"),

    // 宿主机编辑。
    hostEditModal: document.getElementById("hostEditModal"),
    hostEditClose: document.getElementById("hostEditClose"),
    hostEditNameInput: document.getElementById("hostEditNameInput"),
    hostEditNoteInput: document.getElementById("hostEditNoteInput"),
    hostEditSaveBtn: document.getElementById("hostEditSaveBtn"),
    hostEditCancelBtn: document.getElementById("hostEditCancelBtn"),

    // 宿主机负载弹窗。
    hostMetricsModal: document.getElementById("hostMetricsModal"),
    hostMetricsTitle: document.getElementById("hostMetricsTitle"),
    hostMetricsClose: document.getElementById("hostMetricsClose"),
    hostMetricsRows: document.getElementById("hostMetricsRows"),
    hostSidecarRows: document.getElementById("hostSidecarRows"),
    hostMetricsTip: document.getElementById("hostMetricsTip"),

    // 通知弹窗（重名/删除补偿提示）。
    hostNoticeModal: document.getElementById("hostNoticeModal"),
    hostNoticeClose: document.getElementById("hostNoticeClose"),
    hostNoticeTitle: document.getElementById("hostNoticeTitle"),
    hostNoticeBody: document.getElementById("hostNoticeBody"),
    hostNoticePrimaryBtn: document.getElementById("hostNoticePrimaryBtn"),
    hostNoticeSecondaryBtn: document.getElementById("hostNoticeSecondaryBtn"),
  };
}
