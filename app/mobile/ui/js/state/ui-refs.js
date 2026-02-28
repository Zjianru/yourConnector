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
    appRoot: document.getElementById("appRoot"),
    topBar: document.getElementById("topBar"),
    opsView: document.getElementById("opsView"),
    chatView: document.getElementById("chatView"),
    mainTabs: document.getElementById("mainTabs"),
    tabOps: document.getElementById("tabOps"),
    tabChat: document.getElementById("tabChat"),

    // 顶部操作。
    connectBtnTop: document.getElementById("connectBtnTop"),
    disconnectBtnTop: document.getElementById("disconnectBtnTop"),
    replaceHostBtnTop: document.getElementById("replaceHostBtnTop"),

    // 配对与总览。
    hostSetupCard: document.getElementById("hostSetupCard"),
    hostOverviewWrap: document.getElementById("hostOverviewWrap"),
    importPairLinkBtn: document.getElementById("importPairLinkBtn"),
    openManualPairBtn: document.getElementById("openManualPairBtn"),
    hostBannerTrack: document.getElementById("hostBannerTrack"),
    hostBannerDots: document.getElementById("hostBannerDots"),
    toolsGroupedList: document.getElementById("toolsGroupedList"),

    // 聊天页。
    chatListPage: document.getElementById("chatListPage"),
    chatDetailPage: document.getElementById("chatDetailPage"),
    chatMessagePage: document.getElementById("chatMessagePage"),
    chatConversationList: document.getElementById("chatConversationList"),
    chatBackBtn: document.getElementById("chatBackBtn"),
    chatMessageBackBtn: document.getElementById("chatMessageBackBtn"),
    chatMessageZoomOutBtn: document.getElementById("chatMessageZoomOutBtn"),
    chatMessageZoomInBtn: document.getElementById("chatMessageZoomInBtn"),
    chatMessageZoomLabel: document.getElementById("chatMessageZoomLabel"),
    chatSelectBtn: document.getElementById("chatSelectBtn"),
    chatDeleteSelectedBtn: document.getElementById("chatDeleteSelectedBtn"),
    chatDetailTitle: document.getElementById("chatDetailTitle"),
    chatMessageTitle: document.getElementById("chatMessageTitle"),
    chatMessageMeta: document.getElementById("chatMessageMeta"),
    chatMessageFullBody: document.getElementById("chatMessageFullBody"),
    chatOfflineHint: document.getElementById("chatOfflineHint"),
    chatMessages: document.getElementById("chatMessages"),
    chatQueueSummary: document.getElementById("chatQueueSummary"),
    chatQueue: document.getElementById("chatQueue"),
    chatComposerMediaTray: document.getElementById("chatComposerMediaTray"),
    chatMediaInput: document.getElementById("chatMediaInput"),
    chatFileInput: document.getElementById("chatFileInput"),
    chatAttachBtn: document.getElementById("chatAttachBtn"),
    chatAttachMenu: document.getElementById("chatAttachMenu"),
    chatAttachMediaBtn: document.getElementById("chatAttachMediaBtn"),
    chatAttachFileBtn: document.getElementById("chatAttachFileBtn"),
    chatRecordBtn: document.getElementById("chatRecordBtn"),
    chatRecordStatus: document.getElementById("chatRecordStatus"),
    chatInput: document.getElementById("chatInput"),
    chatSendBtn: document.getElementById("chatSendBtn"),
    chatStopBtn: document.getElementById("chatStopBtn"),

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

    // 报告查看弹窗。
    reportViewerModal: document.getElementById("reportViewerModal"),
    reportViewerTitle: document.getElementById("reportViewerTitle"),
    reportViewerClose: document.getElementById("reportViewerClose"),
    reportViewerPath: document.getElementById("reportViewerPath"),
    reportViewerProgressWrap: document.getElementById("reportViewerProgressWrap"),
    reportViewerProgressBar: document.getElementById("reportViewerProgressBar"),
    reportViewerProgressLabel: document.getElementById("reportViewerProgressLabel"),
    reportViewerError: document.getElementById("reportViewerError"),
    reportViewerBody: document.getElementById("reportViewerBody"),
  };
}
