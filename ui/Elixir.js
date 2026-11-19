// Elixir: opens a regular DSM window with the Elixir page inside.
Ext.ns("Elixir");

Ext.define("Elixir.AppInstance", {
	extend: "SYNO.SDS.AppInstance",
	appWindowName: "Elixir.AppWindow"
});

Ext.define("Elixir.AppWindow", {
	extend: "SYNO.SDS.AppWindow",

	constructor: function(config) {
		this.appInstance = config.appInstance;
		config = Ext.apply({
			resizable: true,
			maximizable: true,
			minimizable: true,
			width: 1180,
			height: 760,
			minWidth: 900,
			minHeight: 620,
			items: [{
				xtype: "box",
				autoEl: {
					tag: "iframe",
					src: "/webman/3rdparty/Elixir/index.html?v=@VERSION@",
					width: "100%",
					height: "100%",
					frameborder: "0",
					style: "background:#04040c"
				}
			}]
		}, config);
		this.callParent([config]);
	}
});
