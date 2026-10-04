# 协议逆向记录

本文记录 `com.yuandao.nicehck` **2.3.8** 的 USB HID 控制协议是如何还原出来的，
以及每一处结论对应的证据。所有结论均来自 APK 静态分析，未使用厂商文档。

## 1. 分析方法

| 步骤 | 工具 | 产出 |
|---|---|---|
| 解包 | `unzip` | `classes.dex`（7.08 MB）、`assets/device-config/*.json` |
| 字符串与类型交叉引用 | 自写 androguard 脚本 | 混淆类名 → 字符串/类型引用索引 |
| 字节码反汇编 | androguard | DEX 指令级证据（常量、跳转、数组字面量） |
| 反编译 | jadx 1.5.3 | Java 级可读代码 |
| 真机描述符 | sysfs | USB 接口、端点、HID 报告描述符 |

APK 经 R8 全量混淆：类名被压成 `Lwb6;`、`Lsc6;` 这类形式，**但字符串常量、
字段名与枚举值完整保留**。因此定位路径是：

1. 从 dex 的字符串池找出含协议语义的常量（如 `"setEqualizer failed: "`）
2. 用指令级 `const-string` 交叉引用定位到具体方法
3. 在方法内读数组字面量（`fill-array-data-payload`），拿到协议字节
4. 用字段名（保留未混淆）与调用关系确认结构语义

关键类对照（混淆名 → 实际作用）：

| 混淆名 | 作用 | 定位依据 |
|---|---|---|
| `wb6` | OTA 升级传输 | 含 `"Unknown USB protocol adapter: "`、`USB_SEND` 日志 |
| `sc6` / `rc6` / `qc6` | 帧编解码器 + 头/体结构 | `header CRC mismatch: expected=0x` |
| `n76` | 设备连接与作业编排 | `"USB HID interface not found; vendorId=0x"` |
| `rn3` | **控制命令控制器** | 实现 `b66`，含全部 `0x4B` 命令构造 |
| `bn5` | EQ 功能实现 | 字符串 `"EqualizerFeatureImpl"` |
| `m84` | EQ 载荷构造 | `bn5` 调用其 `i(...)` |
| `cq1` | EQ 能力参数（频率/增益/Q 范围） | 11 字段构造器 |
| `zm5` | 响应操作码枚举 | 枚举名 `MIC_GAIN`/`DAC`/`EQ`/... 保留 |
| `oe6` / `fa5` / `s91` | 协议适配器注册表 | `"ttgk_v1"` 等字面量 |
| `qj4` | 命令名映射 | `HANDSHAKE(0x...` 字符串 |

## 2. 物理层

真机 `lsusb`：

```
Bus 001 Device 005: ID 3302:c200 YUANDAO 10th Anniversary Edition
```

USB 描述符（`/sys/bus/usb/devices/1-1/`）：

| 接口 | 类别 | 驱动 | 端点 |
|---|---|---|---|
| 0 | 0x01 Audio / 0x01 | `snd-usb-audio` | — |
| 1 | 0x01 Audio / 0x02 | `snd-usb-audio` | — |
| 2 | 0x01 Audio / 0x02 | `snd-usb-audio` | 1 |
| 3 | **0x03 HID** | `usbhid` | OUT `0x05` / IN `0x86`，中断，64 字节 |

HID 报告描述符（`report_descriptor`，46 字节）：

```
05 0C        Usage Page (Consumer)
09 01        Usage (Consumer Control)
A1 01        Collection (Application)
85 03          Report ID (3)          ← 多媒体键，走 hid-generic
...
06 01 FF       Usage Page (Vendor Defined 0xFF01)
85 4B          Report ID (0x4B)       ← 控制通道
75 08 95 3F    Report Size 8, Count 63
09 01 81 03    Input   (63 字节)
95 3F 09 02 91 02  Output
85 54          Report ID (0x54)       ← OTA 通道
95 3F 09 03 81 03  Input
95 3F 09 04 91 02  Output
C0
```

**结论**：控制通道 = 报告 ID `0x4B` + 63 字节载荷 = 64 字节报告。
这与 App 里 `new byte[64]` 且 `bArr[0] = 75` 完全一致（`75 = 0x4B`）。

## 3. 控制报告格式

来自 `bn5.a`、`bn5.b`、`m84.i`、`sn3.a`、`xx4`：

```
写：  [0x4B] [0x01] [opcode] [arg] [ payload… ]
读：  [0x4B] [0x80] [opcode] [ payload… ]
应答：[0x4B] [0x80] [opcode] [len] [ payload… ]
```

### 命令表

`zm5` 枚举（字段名未混淆，值来自构造器）：

| 操作码 | 名称 | 方向 | 语义 |
|---|---|---|---|
| `0x02` | `MIC_GAIN` | 读写 | 麦克风增益，signed Q8 dB，限 ±12 |
| `0x03` | `DAC` | 写 | EQ 索引 / offset 落盘 |
| `0x04` | `APPLY` | 写 | 提交当前暂存 |
| `0x09` | `EQ` | 读写 | 参量均衡段 |
| `0x0C` | `VERSION` | 读 | 固件版本（ASCII，NUL 结尾） |
| `0x11` | `FILTER_TYPE` | 读写 | 数字滤波 |
| `0x16` | `CHANNEL_BALANCE` | 读写 | 声道平衡 |
| `0x19` | `GAIN_LEVEL` | 读写 | 输出增益档位 |
| `0x1D` | `WORK_MODE` | 读写 | Class-H / Class-AB |
| `0x85` | `DEVICE_VOLUME` | 读写 | 设备音量 0–100 |
| `0x99` | `UNKNOWN` | — | 兜底 |

### 各命令原始字节

直接摘自反编译代码：

```java
// 读 EQ 段（bn5.a）— 索引在 byte 5
bArr[0]=75; bArr[1]=-128; bArr[2]=9; bArr[5]=(byte)i;

// 写 EQ（bn5.b）— 尾部哨兵
{75, 1, 1}          // DAC 索引
{75, 1, 3, 2, lo, hi}  // offset = 预设索引 * 256
{75, 1, 4}          // APPLY

// 麦克风增益（rn3.k）
{75, 1, 2, 2, lo, hi}

// 声道平衡（m84.h：左/右各一条）
{75, 1, 22, 4, channel, lo, hi}

// 读声道平衡（xx4）
{75, -128, 22, 4, 0}  和  {75, -128, 22, 4, 1}

// 滤波（rn3.m）
{75, 1, 17, 1, filterByte}

// 增益档位（rn3.r）
{75, 1, 25, 1, levelByte}

// 工作模式（rn3.z / sn3）
{75, 1, 29, 1, modeByte}

// 设备音量（rn3.d）
{75, 1, -123, 1, percent}     // -123 = 0x85

// 读音量 / 读滤波
{75, -128, -123}
{75, -128, 17}
```

### 枚举取值

```java
// ym5 数字滤波
FAST_LL=1, FAST_PC=2, SLOW_LL=3, SLOW_PC=4, NON_OS=5, UNKNOWN=6

// t62 增益档位
Low=0, Medium=1, Heigh=2

// do6 工作模式
CLASS_H=0, CLASS_AB=1
```

## 4. EQ 段载荷

`m84.i` 逐字节构造，64 字节数组：

| 偏移 | 内容 |
|---|---|
| 0 | `0x4B` 报告 ID |
| 1 | `0x01` 写 |
| 2 | `0x09` EQ 操作码 |
| 3 | `0x21` (33) 子命令：写段 |
| 4 | `0x00` 声道（固定 0） |
| 5 | 段索引 |
| 6–7 | `0x00` |
| 8–11 | `b0/a0`，signed Q30 LE |
| 12–15 | `b1/a0` |
| 16–19 | `b2/a0` |
| 20–23 | `-a1/a0` |
| 24–27 | `-a2/a0` |
| 28–29 | 中心频率 Hz，u16 LE |
| 30–31 | 增益 dB，signed Q8 |
| 32–33 | Q，signed Q8 |
| 34 | `0x02` 滤波器类型（peaking） |
| 35 | `0x00` |
| 36 | 预设索引 |

整段写完后追加：

```java
{75, 1, 10, 4, 0, 0, -1, -1}    // 0x0A 提交，-1,-1 = 0xFFFF
```

**Q8 定点**：`eg3.Z(f * 256.0f)`，即 `round(v * 256)`。
**Q30 系数**：`eg3.Y(v * 1.073741824E9)`，即 `round(v * 2^30)`。

**舍入**：`eg3.Y` / `eg3.Z` 用 Kotlin `Math.round(double)`，其定义是
`floor(x + 0.5)`——与 Rust `f64::round`（负数半数远离零）在 `-2.5` 这类值上不同。
本实现用 `(v + 0.5).floor()` 精确对齐。

### 系数算法

`ji0.A(d,d,d,d,z)` 是标准 RBJ audio-EQ-cookbook peaking 滤波器：

```
w0    = 2π·f0/Fs
A     = √(10^(gain/20))
alpha = sin(w0)/(2Q)

b0 = 1 + alpha·A     b1 = -2cos(w0)     b2 = 1 - alpha·A
a0 = 1 + alpha/A     a1 = -2cos(w0)     a2 = 1 - alpha/A
```

返回前按 `a0` 归一化。下发时 `a1`、`a2` **取负**。

### 读回应答解析

`rn3.l` 声明式分发：

| 操作码 | 解析 |
|---|---|
| `0x02` | byte4..5 signed Q8 → 增益，限 ±12 |
| `0x03` | byte4..5 u16 / 256 → EQ offset |
| `0x09` | byte36 = 预设索引；byte5 = 段号；byte28..29 频率；byte30..31 增益；byte32..33 Q |
| `0x0C` | byte4 起 ASCII，NUL 结尾 |
| `0x11` | byte4 → 滤波枚举 |
| `0x16` | byte4 = 声道，byte5..6 signed Q8；值为 0 时忽略 |
| `0x19` | byte4 → 增益档位 |
| `0x1D` | byte4 → 工作模式 |
| `0x85` | byte4，限 0–100 |

## 5. 固件（OTA）帧格式

来自 `sc6` / `rc6` / `qc6`，走报告 ID `0x54`。

```
偏移  长度  字段
   0     4  魔数 'B' 'U' 'X' 'X'（LE 0x58585542）
   4     2  命令 u16 LE
   6     1  flags
   7     1  序列号
   8     2  体长度 u16 LE
  10     4  头部 CRC-32（覆盖 byte 0..10）
  14     n  体
14+n     4  体 CRC-32（体非空时才有）
```

**魔数更正记录**：`rc6.a` 初始化为整型 `1482184002`。最初误算成 `0x58595552`
（`RUYX`），单元测试按其小端字节断言后立刻失败，实为 **`0x58585542` → `BUXX`**。
厂商解析器正是逐字节搜 `66, 85, 88, 88`（`B`,`U`,`X`,`X`）来重新同步的，
这与 `sc6.c` 的状态机一致，交叉验证了该结论。

### CRC-32

`rh0.z(int, byte[])`：累加器初值 `-1`，逐字节

```java
v1 ^= (b & 0xFF);
for (int i = 0; i < 8; i++) {
    v1 = (v1 & 1) != 0 ? (v1 >>> 1) ^ 0xEDB88320 : v1 >>> 1;
}
return ~v1;
```

即标准 CRC-32/ISO-HDLC（zlib）。已用 `""→0`、`"123456789"→0xCBF43926`、
`"The quick brown fox..."→0x414FA339` 验证。

### OTA 命令

`qj4.f(short)`：

| 值 | 名称 |
|---|---|
| `0x10` | `HANDSHAKE` |
| `0x20` | `SET_FLASH_WRITE_AREA` |
| `0x21` | `WRITE_FLASH_DATA` |
| `0x24` | `END_AND_REBOOT` |

`wb6` 是 OTA 升级器（含镜像 CRC 校验与分段写入），本工具暂不实现刷写。

## 6. 设备配置

`assets/device-config/*.json` 是 App 自带的设备目录，含 VID/PID、协议变体与
完整功能参数。本工具逐字节嵌入，用于：

- 按 VID/PID 识别型号
- 取 EQ 能力范围（段数、频率/增益/Q 上下限、采样率）
- 列出出厂预设及其曲线

例：`device-49664.json`（十周年纪念版）→ `frequencyNumber: 8`、
`sampleRate: 96000`、7 个预设，默认段位
`50, 200, 500, 1000, 2000, 5000, 10000, 15000 Hz`。

## 7. 未验证与待确认项

诚实标注当前状态：

| 项 | 状态 |
|---|---|
| 报告 ID、端点、64 字节长度 | ✅ 真机描述符确认 |
| CRC-32 算法 | ✅ 已知向量确认 |
| EQ 载荷布局、系数算法 | ✅ 代码逐字节推导 + 单元测试 |
| 设备枚举（VID/PID/型号匹配） | ✅ 真机确认，见 §7.1 |
| 权限链路（udev/uaccess） | ✅ 机制在本机验证，见 §7.2 |
| Nix 打包与 NixOS 模块 | ✅ 求值 + 构建 + 规则落盘验证，见 §7.3 |
| GUI 启动与窗口创建 | ✅ 真机确认（Wayland/niri） |
| **读写往返（真机）** | ⏳ 待 `hidraw` 权限放行后验证 |
| 应答是否按段回 8 条 | ⏳ 待真机确认 |
| 写报告是否需要补齐到 64 字节 | ⏳ 已按 App 行为补齐，待真机确认 |
| 蓝牙型号（OriG in） | ❌ 不在范围 |
| 固件刷写 | ❌ 未实现（风险高） |

传输层已按 App 行为实现：每条暂存命令间隔 20 ms，应答超时 1000 ms，
读取循环在所有报告到达或超时后返回。

### 7.1 真机枚举（已确认）

`nicehck list` 在实机上正确识别：

```
/dev/hidraw3  3302:C200  YUANDAO 10th Anniversary Edition  [model: 10th Anniversary Edition (id 49664)]
```

对应 sysfs 链路，确认厂商接口是 4 个接口中的第 3 个（0–2 为 `snd-usb-audio`）：

```
/sys/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1:1.3/0003:3302:C200.0004/hidraw/hidraw3
  1-1:1.3           bInterfaceClass=03        ← vendor HID 接口
  1-1               idVendor=3302 idProduct=c200
```

`enumerate()` 通过读取 `/sys/class/hidraw/*/device/uevent` 的 `HID_ID` 拿到
VID/PID，因此不依赖 `lsusb` 或 libusb，也不需要 root 就能枚举。

### 7.2 权限链路（机制已验证）

实机现象：`/dev/hidraw3` 为 `crw------- root root`，ACL 中只有 root，
普通用户打开得到 `EACCES`。这不是程序缺陷，而是内核默认策略：

- `50-udev-default.rules` 只对 hidraw 做 `IMPORT{builtin}="hwdb"`，不改权限
- `70-uaccess.rules` 的内置白名单只覆盖声卡、摄像头、手柄等类别，
  厂商自定义 HID 不在其中，所以设备只拿到 `:seat:` 标签，**没有 `:uaccess:`**
- 真正执行 ACL 的是 `73-seat-late.rules` 里的
  `TAG=="uaccess", RUN{builtin}+="uaccess"`

因此规则文件名必须排在 `73-seat-late.rules` 之前，本项目用 `70-` 前缀。

已在实机验证的旁证：`/dev/snd/*` 带有 `user:mooling` ACL，说明本机的
logind uaccess 机制工作正常，缺的只是针对 `3302` 的规则。

### 7.3 Nix 打包（已验证）

- `nicehck-udev-rules` 的规则落在 `$out/etc/udev/rules.d/70-nicehck.rules`，
  这正是 NixOS `udevRulesFor` 扫描的路径
  （`$package/{etc,lib}/udev/rules.d/*.rules`）
- 把**二进制包**传给 `services.udev.packages` 不会生效，必须传 `udevRules` 包
- `nixosModules.default` 在真实 NixOS 配置中求值通过，
  `nicehck-udev-rules` 出现在 `services.udev.packages`，且规则被复制进
  `environment.etc."udev/rules.d"` 的 store 产物中，排序位于
  `73-seat-late.rules` 之前
- `udevadm verify` 对规则文件报 `Success: 1 Fail: 0`

