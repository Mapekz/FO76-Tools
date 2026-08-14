# LVLI Leveled-List Health Sweep

Scanned **10962** LVLI records.
- Rule A (Use-First-Match order-starvation): **26**
- Rule B (bundle-name uniform-pick): **27**
- Rule C (suspected level-tier starvation, UNVERIFIED): **165**
- Rule D (overlapping-gate reward ladder, no Use All/First): **28**

## Rule A — Use-First-Match order-starvation (confirmed mechanism)

`Use First Object That Matches All Conditions` walks entries in list order and takes the first whose Conditions pass. An entry below is only reachable when every entry above it fails its check — flagged here when an early entry has no Conditions at all, or a `GetRandomPercent` threshold >= 95.

### `LL_Wastelander_Outfit` (0x001C2699)
- entry 0 (`LLS_Clothes_Wastelander_NotPlayable`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 1 (`LLS_Clothes_Wastelander_NotPlayable`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 2 (`LLS_Clothes_Wastelander_With_Headwear_NotPlayable`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Headwear_BloodEagle_NONPLAYABLE` (0x00565BCC)
- entry 0 (`LL_Headwear_BloodEagle_Face_Covered_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Headwear_Cultist_NONPLAYABLE` (0x0056862C)
- entry 0 (`LL_Headwear_Cultist_Face_Covered_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Headwear_Raider_NONPLAYABLE` (0x00572C07)
- entry 0 (`LL_Headwear_Raider_Face_Covered_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `ATX_Resources_Collectron_Communist_Tools` (0x00591A38)
- entry 0 (`Hammer01`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 6 entries below it
- entry 1 (`Sickle`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 5 entries below it
- entry 2 (`Shovel`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 4 entries below it
- entry 3 (`Pitchfork`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 4 (`Pickaxe`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 5 (`WoodCuttingAxe`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Recipes_Mods_Weapons_Melee_AllRegions_PreWar` (0x005A2D1A)
- entry 0 (`miscmod_mod_melee_ChineseOfficerSword_ShockAndSerrated`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 19 entries below it
- entry 1 (`DLC04_miscmod_mod_melee_DLC04_CommieWhacker_BladesLarge`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 18 entries below it
- entry 2 (`miscmod_mod_melee_Hatchet_ElectroFusion`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 17 entries below it
- entry 3 (`miscmod_mod_Chainsaw_Bar_Bow_Long`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 16 entries below it
- entry 4 (`DLC04_miscmod_mod_melee_Sledgehammer_ExtraHeavyHead_Rocket_Spikes`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 15 entries below it
- entry 5 (`miscmod_mod_melee_Machete_Sacrificial`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 14 entries below it
- entry 6 (`DLC04_miscmod_mod_melee_Sledgehammer_ExtraHeavyHead_Rocket_Blades`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 13 entries below it
- entry 7 (`DLC04_miscmod_mod_melee_BaseballBat_Rocket_SpikesLarge_Heated`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 12 entries below it
- entry 8 (`miscmod_mod_melee_Powerfist_Heated`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 11 entries below it
- entry 9 (`DLC04_miscmod_mod_melee_Sledgehammer_ExtraHeavyHead_Rocket_Blades_Heated`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 10 entries below it
- entry 10 (`miscmod_mod_melee_Ripper_BladesLarge`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 9 entries below it
- entry 11 (`miscmod_mod_Chainsaw_Flamer`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 8 entries below it
- entry 12 (`miscmod_mod_melee_WalkingCane_SpikesSmall`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 7 entries below it
- entry 13 (`miscmod_mod_melee_Switchblade_Serrated`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 6 entries below it
- entry 14 (`miscmod_mod_melee_WalkingCane_Spikes`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 5 entries below it
- entry 15 (`DLC04_miscmod_mod_melee_BaseballBat_Rocket`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 4 entries below it
- entry 16 (`DLC04_miscmod_mod_melee_BaseballBat_Heated`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 17 (`miscmod_mod_melee_ChineseOfficerSword_Shock`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 18 (`miscmod_mod_melee_Pitchfork_Flamer`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Recipes_Weapons_Ranged_AllRegions_PreWar` (0x005A2D1C)
- entry 0 (`Recipe_Weapon_Ranged_LaserGun`) is always-true (no Conditions) (Minimum Level 5 <= next entry's 5) — starves 22 entries below it
- entry 1 (`Recipe_Weapon_Ranged_44`) is always-true (no Conditions) (Minimum Level 5 <= next entry's 5) — starves 21 entries below it
- entry 2 (`Recipe_Weapon_Ranged_PumpActionShotgun`) is always-true (no Conditions) (Minimum Level 5 <= next entry's 10) — starves 20 entries below it
- entry 3 (`Recipe_Weapon_Ranged_10mmSMG`) is always-true (no Conditions) (Minimum Level 10 <= next entry's 10) — starves 19 entries below it
- entry 4 (`Recipe_Weapon_Ranged_SingleActionRevolver`) is always-true (no Conditions) (Minimum Level 10 <= next entry's 15) — starves 18 entries below it
- entry 5 (`Recipe_Weapon_Ranged_Crossbow`) is always-true (no Conditions) (Minimum Level 15 <= next entry's 15) — starves 17 entries below it
- entry 6 (`Recipe_Weapon_Ranged_M79`) is always-true (no Conditions) (Minimum Level 15 <= next entry's 15) — starves 16 entries below it
- entry 7 (`Recipe_Weapon_Ranged_PlasmaGun`) is always-true (no Conditions) (Minimum Level 15 <= next entry's 15) — starves 15 entries below it
- entry 8 (`Recipe_Weapon_Ranged_SubmachineGun`) is always-true (no Conditions) (Minimum Level 15 <= next entry's 20) — starves 14 entries below it
- entry 9 (`Recipe_Weapon_Ranged_CombatRifle_MQ`) is always-true (no Conditions) (Minimum Level 20 <= next entry's 20) — starves 13 entries below it
- entry 10 (`Recipe_Weapon_Ranged_Combatshotgun`) is always-true (no Conditions) (Minimum Level 20 <= next entry's 20) — starves 12 entries below it
- entry 11 (`Recipe_Weapon_Ranged_Revolver`) is always-true (no Conditions) (Minimum Level 20 <= next entry's 25) — starves 11 entries below it
- entry 12 (`Recipe_Weapon_Ranged_50CalMachineGun`) is always-true (no Conditions) (Minimum Level 25 <= next entry's 25) — starves 10 entries below it
- entry 13 (`Recipe_Weapon_Ranged_LeverGun`) is always-true (no Conditions) (Minimum Level 25 <= next entry's 25) — starves 9 entries below it
- entry 14 (`Recipe_Weapon_Ranged_Cryolater`) is always-true (no Conditions) (Minimum Level 25 <= next entry's 25) — starves 8 entries below it
- entry 15 (`Recipe_Weapon_Ranged_GatlingLaser`) is always-true (no Conditions) (Minimum Level 25 <= next entry's 30) — starves 7 entries below it
- entry 16 (`Recipe_Weapon_Ranged_HarpoonGun`) is always-true (no Conditions) (Minimum Level 30 <= next entry's 30) — starves 6 entries below it
- entry 17 (`Recipe_Weapon_Ranged_Flamer`) is always-true (no Conditions) (Minimum Level 30 <= next entry's 30) — starves 5 entries below it
- entry 18 (`Recipe_Weapon_Ranged_GatlingPlasma`) is always-true (no Conditions) (Minimum Level 30 <= next entry's 30) — starves 4 entries below it
- entry 19 (`Recipe_Weapon_Ranged_MG42`) is always-true (no Conditions) (Minimum Level 30 <= next entry's 35) — starves 3 entries below it
- entry 20 (`Recipe_Weapon_Ranged_MiniGun_MQ`) is always-true (no Conditions) (Minimum Level 35 <= next entry's 35) — starves 2 entries below it
- entry 21 (`Recipe_Weapon_Ranged_GaussRifle`) is always-true (no Conditions) (Minimum Level 35 <= next entry's 40) — starves 1 entry below it
### `LL_Recipes_Mods_Weapons_Ranged_AllRegions_PreWar` (0x005A2D1D)
- entry 0 (`recipe_DLC03_mod_HarpoonGun_Magazine_Barbed`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 49 entries below it
- entry 1 (`recipe_DLC03_mod_HarpoonGun_Magazine_Flechette`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 48 entries below it
- entry 2 (`recipe_DLC03_mod_LeverGun_SCOPE_Longscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 47 entries below it
- entry 3 (`recipe_DLC03_mod_LeverGun_SCOPE_MediumScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 46 entries below it
- entry 4 (`recipe_DLC03_mod_LeverGun_SCOPE_shortscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 45 entries below it
- entry 5 (`recipe_mod_Fatman_Barrel_MIRV`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 44 entries below it
- entry 6 (`recipe_mod_Flamer_Receiver_TankNapalm`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 43 entries below it
- entry 7 (`recipe_mod_GatlingLaser_Receiver_Burning-CritDMG`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 42 entries below it
- entry 8 (`recipe_mod_GatlingLaser_Receiver_Burning-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 41 entries below it
- entry 9 (`recipe_mod_GaussRifle_SCOPE_longscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 40 entries below it
- entry 10 (`recipe_mod_GaussRifle_SCOPE_MediumScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 39 entries below it
- entry 11 (`recipe_mod_GaussRifle_SCOPE_shortscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 38 entries below it
- entry 12 (`recipe_mod_HuntingRifle_Receiver_AmmoConv38`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 37 entries below it
- entry 13 (`recipe_mod_HuntingRifle_Receiver_AmmoConv38-CritDMG`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 36 entries below it
- entry 14 (`recipe_mod_HuntingRifle_Receiver_AmmoConv38-Damage`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 35 entries below it
- entry 15 (`recipe_mod_HuntingRifle_Receiver_AmmoConv38-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 34 entries below it
- entry 16 (`recipe_mod_HuntingRifle_Receiver_AmmoConv50`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 33 entries below it
- entry 17 (`recipe_mod_HuntingRifle_Receiver_AmmoConv50-CritDMG`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 32 entries below it
- entry 18 (`recipe_mod_HuntingRifle_Receiver_AmmoConv50-Damage`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 31 entries below it
- entry 19 (`recipe_mod_HuntingRifle_Receiver_AmmoConv50-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 30 entries below it
- entry 20 (`recipe_mod_HuntingRifle_Receiver_FastTrigger-AmmoConv38`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 29 entries below it
- entry 21 (`recipe_mod_HuntingRifle_Receiver_FastTrigger-AmmoConv50`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 28 entries below it
- entry 22 (`recipe_mod_HuntingRifle_Receiver_ScorchedKiller`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 27 entries below it
- entry 23 (`recipe_mod_HuntingRifle_SCOPE_LongScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 26 entries below it
- entry 24 (`recipe_mod_HuntingRifle_SCOPE_MediumScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 25 entries below it
- entry 25 (`recipe_mod_HuntingRifle_SCOPE_ShortScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 24 entries below it
- entry 26 (`recipe_mod_LaserGun_Barrel_Spinning_Recoil-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 23 entries below it
- entry 27 (`recipe_mod_LaserGun_Receiver_Burning-CritDMG`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 22 entries below it
- entry 28 (`recipe_mod_LaserGun_Receiver_Burning-FastTrigger`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 21 entries below it
- entry 29 (`recipe_mod_LaserGun_Receiver_Burning-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 20 entries below it
- entry 30 (`recipe_mod_LaserGun_SCOPE_longscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 19 entries below it
- entry 31 (`recipe_mod_LaserGun_SCOPE_MediumScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 18 entries below it
- entry 32 (`recipe_mod_LaserGun_SCOPE_shortscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 17 entries below it
- entry 33 (`recipe_mod_Minigun_BarrelMinigun_TriBarrel`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 16 entries below it
- entry 34 (`recipe_mod_MissileLauncher_Scope_ScopeLong_NV`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 15 entries below it
- entry 35 (`recipe_mod_MissileLauncher_Scope_TargetingBox`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 14 entries below it
- entry 36 (`recipe_mod_MissileLauncher_TubeBarrel_Quad`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 13 entries below it
- entry 37 (`recipe_mod_MissileLauncher_TubeBarrel_Triple`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 12 entries below it
- entry 38 (`recipe_mod_PlasmaGun_Barrel_Flamer_HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 11 entries below it
- entry 39 (`recipe_mod_PlasmaGun_Barrel_Flamer_Recoil`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 10 entries below it
- entry 40 (`recipe_mod_PlasmaGun_Barrel_Shotgun_HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 9 entries below it
- entry 41 (`recipe_mod_PlasmaGun_Barrel_Shotgun_Recoil`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 8 entries below it
- entry 42 (`recipe_mod_PlasmaGun_Barrel_Spin_Recoil-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 7 entries below it
- entry 43 (`recipe_mod_PlasmaGun_Receiver_Burning-CritDMG`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 6 entries below it
- entry 44 (`recipe_mod_PlasmaGun_Receiver_Burning-HipAccuracy`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 5 entries below it
- entry 45 (`recipe_mod_PlasmaGun_SCOPE_Longscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 4 entries below it
- entry 46 (`recipe_mod_PlasmaGun_SCOPE_MediumScope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 47 (`recipe_mod_PlasmaGun_SCOPE_shortscope_NV_Base`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 48 (`recipe_mod_SubmachineGun_Receiver_Automatic1_and_ArmorPiercing`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `zzz_NOTinUSE_ATX_Resources_Collectron_Fasnacht_Party` (0x005AD5A8)
- entry 1 (`ATX_Resources_Collectron_Santa_Toys`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 2 (`ATX_Resources_Collectron_Santa_Sweets`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 3 (`c_Fertilizer_scrap`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Ammo_Loot_Container` (0x00621921)
- entry 0 (`LLS_Ammo_Loot_Contextual`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LLI_Outfit_Refugee` (0x00654C52)
- entry 0 (`LL_Outfit_Refugee_Normal`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Clothes_MunicipalWorker_Nurse_XPD_AC_NONPLAYABLE` (0x006CC07A)
- entry 0 (`Clothes_NurseUniform1_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 10 entries below it
- entry 1 (`Clothes_AsylumWorkerUniformYellow_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 9 entries below it
- entry 2 (`Clothes_AsylumWorkerUniformWhiteDirty_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 8 entries below it
- entry 3 (`Clothes_AsylumWorkerUniformWhite_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 7 entries below it
- entry 4 (`Clothes_AsylumWorkerUniformWeathered_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 6 entries below it
- entry 5 (`Clothes_AsylumWorkerUniformRed_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 5 entries below it
- entry 6 (`Clothes_AsylumWorkerUniformPink_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 4 entries below it
- entry 7 (`Clothes_AsylumWorkerUniformGreen_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 8 (`Clothes_AsylumWorkerUniformForest_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 9 (`Clothes_AsylumWorkerUniformBrown_NONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Drink_Alcohol_VintageLeadChampagne_MJM` (0x006F6193)
- entry 0 (`Brew_LeadChampagneVintage`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 1 (`Brew_LeadChampagneVintage`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LL_Drink_Alcohol_HighVoltageHefe_MJM` (0x006F61A2)
- entry 0 (`Brew_HighVoltageHefeFerm`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 1 (`Brew_HighVoltageHefeFerm`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `NPE_LL_Headwear_VRBully_NONPLAYABLE` (0x00709C31)
- entry 0 (`NPE_Headwear_VaultGirl_RaiderNONPLAYABLE`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `LLS_Ammo_Loot_SupermutantSuicider_MiniNuke` (0x007ACEEB)
- entry 0 (`AmmoFatManMiniNuke`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `ATX_Resources_Collectron_BivBev` (0x007C729C)
- entry 0 (`ATX_Resources_Collectron_BivBev_Materials`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 1 (`ATX_Resources_Collectron_BivBev_Beer`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `Burn_LL_GunVendor_Recipes_Weapons` (0x0084C558)
- entry 0 (`Recipe_Weapon_Melee_DeathclawGauntlet`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 4 entries below it
- entry 1 (`Recipe_Weapon_Melee_WarDrum`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 2 (`Recipe_Weapon_Melee_BaseballBat`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 3 (`Recipe_Weapon_Ranged_Crossbow`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `Burn_LL_GunVendor_Recipes_Weapon_Mods` (0x0084C560)
- entry 0 (`recipe_mod_Chainsaw_Bar_Bow_Long`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 5 entries below it
- entry 1 (`recipe_mod_melee_DeathclawGauntlet_Hook`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 4 entries below it
- entry 2 (`W05_recipe_mod_RegularBow_Scope_IronSights_Glow`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 3 entries below it
- entry 3 (`recipe_mod_melee_Shishkebab_ExtraFlameJets`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it
- entry 4 (`recipe_mod_melee_Ripper_BladesLarge`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `RA_LL_Rewards_General_AidItems_Rare` (0x0086A8C4)
- entry 0 (`RA_LL_Rewards_General_AidItems_Rare_Need`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `RA_LL_Rewards_General_AidItems_Common` (0x0086A8C5)
- entry 0 (`RA_LL_Rewards_General_AidItems_Common_Need`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
### `HTO_crLLS_Rewards_Legendary_Boss_Armor` (0x0088851E)
- entry 0 (`HTO_LegendaryItems_Armor_Rank4`) is near-certain (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
  - conditions: GetRandomPercent Less Than Or Equal To HTO_LCP_LegendaryReward_ChanceNone_4StarBossDrop (=100)
### `HTO_crLLS_Rewards_Legendary_Boss_PowerArmor` (0x00888520)
- entry 0 (`HTO_LegendaryItems_PowerArmor_Rank4`) is near-certain (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
  - conditions: GetRandomPercent Less Than Or Equal To HTO_LCP_LegendaryReward_ChanceNone_4StarBossDrop (=100)
### `HTO_crLLS_Rewards_Legendary_Boss_Weapons_Melee` (0x00888522)
- entry 0 (`HTO_LegendaryItems_Weapons_Melee_Rank4`) is near-certain (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
  - conditions: GetRandomPercent Less Than Or Equal To HTO_LCP_LegendaryReward_ChanceNone_4StarBossDrop (=100)
### `HTO_crLLS_Rewards_Legendary_Boss_Weapons_Ranged` (0x00888524)
- entry 0 (`HTO_LegendaryItems_Weapons_Ranged_Rank4`) is near-certain (Minimum Level 1 <= next entry's 1) — starves 1 entry below it
  - conditions: GetRandomPercent Less Than Or Equal To HTO_LCP_LegendaryReward_ChanceNone_4StarBossDrop (=100)
### `ATX_Resources_Collectron_LegendaryGear_VeryRare` (0x008A52C5)
- entry 0 (`ATX_Resources_LL_Collectron_Peppino_LegendaryGear_Rank1`) is always-true (no Conditions) (Minimum Level 1 <= next entry's 1) — starves 2 entries below it

## Rule B — bundle-name uniform-pick (heuristic)

Multi-entry list with neither `Use All` nor `Use First Match` set — the engine picks ONE entry uniformly at random and applies its chance-none. Normal for "pick one of N variants" lists; flagged here only because the EditorID/Name suggests the list is meant to hand out a full set/bundle rather than one item.

- `workshop_LL_Structure_Barn_WallFull` (0x00088246) — 3 entries
- `workshop_LL_Structure_Barn_WallHalf` (0x0008824C) — 3 entries
- `workshop_LL_Structure_Wrhs_WallFull` (0x00088267) — 3 entries
- `workshop_LL_Structure_Brick_WallFull` (0x00089D9D) — 5 entries
- `RNG_ToxicShrubsFernCollections` (0x001230B3) — 7 entries
- `workshop_LL_WallDecor_SmallLetters` (0x003D5B33) — 41 entries
- `ATX_workshop_LL_Appliances_WasherDryer` (0x00613999) — 4 entries
- `E09D_LL_MW_PokerSet` (0x0067AEE7) — 6 entries
- `SCORE_S11_workshop_LL_PlanterKit_RoughPatch` (0x0067C2B2) — 4 entries
- `SCORE_S13_workshop_LL_Misc_MovieSetFacade` (0x0069F3E7) — 5 entries
- `RNG_Storm_ToxicShrubsFernCollections` (0x006DC31F) — 5 entries
- `RNG_Storm_Crpt_ToxicShrubsFernCollections` (0x006DC391) — 5 entries
- `ATX_workshop_LL_FloorDecor_MiniatureSet_Alien` (0x0075F739) — 6 entries
- `E02A_MeatWeek_LLS_AllRewardsKnown` (0x0075FBF6) — 6 entries
- `Fishing_LLS_FishCollection_Forest_Uncommon` (0x007D4991) — 3 entries
- `Fishing_LLS_FishCollection_Ash_Uncommon` (0x007D499B) — 4 entries
- `Fishing_LLS_FishCollection_Cranberry_Uncommon` (0x007D49A2) — 3 entries
- `Fishing_LLS_FishCollection_Mire_Uncommon` (0x007D49A7) — 3 entries
- `Fishing_LLS_FishCollection_SavageDivide_Uncommon` (0x007D49AA) — 4 entries
- `Fishing_LLS_FishCollection_Skyline_Uncommon` (0x007D49B1) — 4 entries
- `Fishing_LLS_FishCollection_Toxic_Uncommon` (0x007D49B4) — 4 entries
- `Fishing_LLS_FishCollection_Generic_Small` (0x008047B3) — 3 entries
- `Fishing_LLS_FishCollection_Generic_Medium` (0x008047B6) — 5 entries
- `ATX_workshop_LL_TrainBridgeSet` (0x0082B89F) — 4 entries
- `Burn_Fishing_LLS_FishCollection_BurningSprings_Uncommon` (0x008482BC) — 3 entries
- `ATX_workshop_LL_FloorDecor_PikeSet` (0x0084A535) — 3 entries
- `ATX_Resources_Collectron_EvidenceCollectionAssistant_SuperCommon` (0x008AD79A) — 21 entries

## Rule C — suspected level-tier starvation (UNVERIFIED for FO76)

**Caveat:** this rule assumes the classic Skyrim/FO4 Creation Engine behavior where, without `Calculate from all levels <= player's level`, entry selection collapses to the single highest Minimum Level <= the player's level, silently excluding lower-Level entries. TES5Edit's FO76 schema defines the flag under the same name but does not confirm this selection-pool behavior for FO76's engine, and a neighboring flag bit in the same source is annotated "Use special formula in skyrim" — this flag family is known to vary by game. Treat every hit below as a **lead to verify**, not a confirmed bug.

- `Container_Loot_Priority_Trunk_Prewar_Boss` (0x000192FA) — Minimum Levels: 1.0, 25.0
- `LLD_Creature_Robot_MrGutsy` (0x0001B346) — Minimum Levels: 1.0, 15.0
- `LLD_Creature_Robot_Assaultron` (0x0001B34D) — Minimum Levels: 1.0, 25.0
- `MTNL01_LL_Armor_Raider_Headgear` (0x0003E0BE) — Minimum Levels: 1.0, 21.0
- `LPI_Weapon_Ranged_10mm` (0x00043225) — Minimum Levels: 1.0, 5.0
- `LL_Weapon_Boss_Ranged_Any` (0x000673AF) — Minimum Levels: 1.0, 5.0, 11.0, 15.0, 17.0, 22.0, 24.0, 26.0, 28.0
- `Container_Loot_Priority_Trunk_Raider_Boss` (0x0006D4B0) — Minimum Levels: 1.0, 20.0
- `LLV_Vendor_Ammo_Rare` (0x0007580B) — Minimum Levels: 1.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0
- `LL_Grenades_25` (0x0007C70D) — Minimum Levels: 1.0, 15.0
- `LLV_Vendor_Weapon_Ranged_Specialty` (0x0008531F) — Minimum Levels: 1.0, 5.0, 15.0, 20.0
- `LLS_Creature_Deathclaw` (0x0008E3AE) — Minimum Levels: 1.0, 31.0, 41.0, 51.0, 61.0, 71.0, 81.0, 91.0
- `LPI_Weapon_Melee_Baton_Shock` (0x000C44C7) — Minimum Levels: 1.0, 5.0, 10.0
- `LPI_Weapon_Ranged_LaserGun` (0x000C9B66) — Minimum Levels: 1.0, 14.0, 15.0, 16.0, 17.0, 19.0, 20.0
- `LPI_Weapon_Ranged_HuntingRifle_Sniper` (0x000D9C9B) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Ranged_CombatShotgun` (0x000E02FF) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Ranged_CombatRifle_Rifle_SemiAuto` (0x000E0300) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Ranged_CombatRifle_Sniper` (0x000E0301) — Minimum Levels: 1.0, 25.0
- `crLLI_Creatures_Grenade_frag_15` (0x000F2E1E) — Minimum Levels: 10.0, 15.0
- `LPI_Weapon_Ranged_AssaultRifle` (0x000F5C50) — Minimum Levels: 1.0, 30.0
- `LPI_Weapon_Ranged_10mm_Auto` (0x00100E34) — Minimum Levels: 1.0, 5.0, 10.0
- `LL_Grenades_15` (0x0011003C) — Minimum Levels: 1.0, 15.0
- `LLE_Creature_FogCrawler` (0x00111626) — Minimum Levels: 1.0, 100.0
- `LLE_Creature_Rabbit` (0x001119D4) — Minimum Levels: 1.0, 100.0
- `LLE_Creature_HermitCrab` (0x001119D5) — Minimum Levels: 1.0, 100.0
- `LLS_Weapons_SavageDivide_High` (0x0014E82B) — Minimum Levels: 1.0, 15.0, 20.0, 30.0, 35.0
- `LPI_Weapon_Ranged_PipeGun_SniperRifle` (0x001642AA) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Ranged_CombatRifle_Rifle_Auto` (0x001790F6) — Minimum Levels: 1.0, 30.0
- `LPI_Weapon_Ranged_CombatShotgun_Rifle_SemiAuto` (0x00186C17) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Ranged_CombatShotgun_Rifle_Auto` (0x00186C18) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Ranged_44` (0x00188A6B) — Minimum Levels: 1.0, 5.0
- `LPI_Weapon_Ranged_Minigun` (0x00188A77) — Minimum Levels: 1.0, 35.0
- `LPI_Weapon_Ranged_MissileLauncher` (0x00188A78) — Minimum Levels: 1.0, 20.0
- `LPI_Weapon_Ranged_SubmachineGun` (0x00188A7B) — Minimum Levels: 1.0, 10.0, 15.0
- `LPI_Weapon_Melee_Baton` (0x00188A94) — Minimum Levels: 1.0, 5.0
- `LPI_Weapon_Melee_Sledgehammer` (0x00188AA1) — Minimum Levels: 1.0, 10.0
- `crLLI_Raider_Grenade_15` (0x0019FFBB) — Minimum Levels: 1.0, 14.0
- `LPI_Weapon_Ranged_Blackpowder_Rifle_Dragon` (0x001A5E83) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Melee_RevolutionarySword` (0x001A607B) — Minimum Levels: 1.0, 10.0
- `LPI_Weapon_Melee_CultistBlade` (0x001A6172) — Minimum Levels: 1.0, 10.0
- `LPI_Weapon_Melee_CultistDagger` (0x001A6298) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Melee_Switchblade` (0x001A62D5) — Minimum Levels: 1.0, 10.0
- `LPI_Weapon_Melee_Drill` (0x001A6321) — Minimum Levels: 1.0, 20.0
- `LLI_BoSSoldierOutfit` (0x00223335) — Minimum Levels: 1.0, 17.0
- `LL_Quest_Reward_Weapon_Any` (0x0022FFBF) — Minimum Levels: 1.0, 5.0, 15.0, 20.0, 25.0, 35.0
- `CUT_crLLI_Supermutant_Autorifle_Boss` (0x0024754D) — Minimum Levels: 1.0, 28.0, 38.0, 54.0
- `CUT_crLLI_Supermutant_Semiauto_Rifle_Boss` (0x0024754E) — Minimum Levels: 1.0, 18.0, 25.0, 54.0
- `ENB_Vendor_ScienceWing_Faction_Enclave` (0x002B8026) — Minimum Levels: 1.0, 40.0
- `Vendor_RandomEncounters` (0x002C5DDA) — Minimum Levels: 1.0, 20.0
- `LLV_Faction_Responders` (0x002C5DEB) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Melee_MoleMinerGauntlet` (0x0033FCB4) — Minimum Levels: 1.0, 20.0
- `Test_InstanceContainer` (0x0034ADC2) — Minimum Levels: 1.0, 10.0, 20.0, 30.0
- `LLS_Creature_FeralGhoul` (0x003533DB) — Minimum Levels: 1.0, 9.0, 15.0, 22.0, 42.0, 52.0, 62.0
- `LLS_Creature_FogCrawler` (0x003533DC) — Minimum Levels: 1.0, 39.0, 51.0, 63.0, 75.0
- `LLS_Creature_Gulper` (0x003533DD) — Minimum Levels: 1.0, 22.0, 34.0, 46.0
- `LLS_Creature_HermitCrab` (0x003533DE) — Minimum Levels: 1.0, 31.0, 41.0, 51.0, 61.0
- `LLS_Creature_Mirelurk` (0x003533DF) — Minimum Levels: 1.0, 12.0, 18.0, 26.0, 34.0, 42.0
- `LLS_MoleMiner` (0x00356BD5) — Minimum Levels: 1.0, 4.0, 8.0, 14.0, 22.0, 30.0, 40.0
- `LLE_MoleMiner` (0x00356BD6) — Minimum Levels: 1.0, 30.0
- `LLS_Creature_Scorched_Ranged` (0x00356BD7) — Minimum Levels: 1.0, 16.0, 22.0, 28.0, 36.0, 42.0, 48.0, 58.0, 68.0
- `LLS_Creature_Assaultron` (0x00356BD9) — Minimum Levels: 1.0, 36.0, 46.0
- `LLE_Creature_Assaultron` (0x00356BDA) — Minimum Levels: 1.0, 25.0
- `LLS_Creature_Bloodbug` (0x00356BDB) — Minimum Levels: 1.0, 10.0, 18.0, 26.0, 34.0, 42.0
- `LLS_Creature_Behemoth` (0x00356BDE) — Minimum Levels: 1.0, 65.0, 80.0, 95.0
- `LLD_Creature_Scorchbeast` (0x0036BA96) — Minimum Levels: 1.0, 30.0, 50.0
- `LLS_Creature_Scorchbeast` (0x0036BA98) — Minimum Levels: 1.0, 16.0, 22.0, 28.0, 36.0, 42.0, 48.0, 58.0, 68.0
- `LPI_Weapon_Ranged_HandmadeGun` (0x0037A22F) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Ranged_HandmadeGun_Rifle_Auto` (0x0037A230) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Ranged_HandmadeGun_Rifle_SemiAuto` (0x0037A231) — Minimum Levels: 1.0, 20.0
- `LPI_Weapon_Ranged_HandmadeGun_Rifle_Sniper` (0x0037A232) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Ranged_HandmadeGun_ShortRifle_SemiAuto` (0x0037A233) — Minimum Levels: 1.0, 15.0
- `LLS_Creature_Liberator` (0x0038232C) — Minimum Levels: 5.0, 18.0, 42.0
- `LLS_Weapon_Ranged_Shotgun_Fallthrough` (0x0039E7E2) — Minimum Levels: 1.0, 15.0, 25.0
- `LLS_Weapon_Ranged_Rifle_Fallthrough` (0x0039E7E4) — Minimum Levels: 1.0, 20.0, 30.0
- `LLS_Loot_Corpse_Weapon_Raider_Ranged` (0x0039EE5C) — Minimum Levels: 1.0, 5.0, 7.0, 14.0, 22.0, 28.0
- `LLI_FreeState_Weapons` (0x003ACCF6) — Minimum Levels: 1.0, 5.0, 7.0, 14.0, 22.0, 28.0
- `LPI_Weapon_Melee_Chainsaw_76` (0x003B81B8) — Minimum Levels: 1.0, 20.0
- `LLV_Vendor_ChemsMeds_Basic` (0x003C24EE) — Minimum Levels: 1.0, 5.0, 10.0
- `LPI_Weapon_Ranged_Blackpowder_Rifle` (0x003CA912) — Minimum Levels: 1.0, 15.0
- `LPI_Weapon_Ranged_Blackpowder_Pistol` (0x003CA913) — Minimum Levels: 1.0, 10.0
- `LLE_NukeResources_Glowing` (0x003D028F) — Minimum Levels: 1.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0
- `LL_Armor_PowerArmor_TSeries_Any_Piece` (0x003D7A14) — Minimum Levels: 1.0, 30.0, 40.0
- `LLS_Creature_Wolf` (0x003DF3CD) — Minimum Levels: 1.0, 20.0, 30.0, 40.0, 50.0
- `LLS_Creature_Supermutant` (0x003DF3F5) — Minimum Levels: 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0
- `LLS_Creature_YaoGuai` (0x003DF3F6) — Minimum Levels: 1.0, 26.0, 31.0, 36.0, 41.0, 46.0, 56.0, 66.0, 76.0
- `LLS_Creature_MirelurkHunter` (0x003DF3F9) — Minimum Levels: 1.0, 12.0, 18.0, 26.0, 34.0, 42.0
- `LLI_Recipes_DiseaseCure_All_COND_Regions` (0x003E08CA) — Minimum Levels: 1.0, 25.0
- `LPI_Weapon_Melee_SkiSword` (0x003EAF2A) — Minimum Levels: 1.0, 15.0
- `LL_Recipes_Armor_Combat_Any_Vendor` (0x003EC602) — Minimum Levels: 1.0, 2.0, 3.0
- `LLS_Recipes_Workshop_Doors_Vendor` (0x003EC64C) — Minimum Levels: 1.0, 10.0, 20.0, 30.0
- `LLS_Recipes_Workshop_Power_Generator_Vendor` (0x003EC64D) — Minimum Levels: 20.0, 30.0
- `LLS_Recipes_Workshop_PowerConnectors_Vendor` (0x003EC64E) — Minimum Levels: 1.0, 20.0
- `LLS_Recipes_Workshop_Walls_Vendor` (0x003EC64F) — Minimum Levels: 1.0, 10.0, 20.0, 30.0
- `LLS_Recipes_Workshop_Water_Vendor` (0x003EC650) — Minimum Levels: 10.0, 20.0, 30.0
- `LLE_Creature_Boss_Large_Dynamic` (0x004342F0) — Minimum Levels: 1.0, 20.0, 30.0, 40.0, 50.0
- `LLE_Creature_Boss_Medium_Dynamic` (0x004342F1) — Minimum Levels: 1.0, 20.0, 30.0, 40.0, 50.0
- `LLE_Creature_Boss_Small_Dynamic` (0x004342F2) — Minimum Levels: 1.0, 20.0, 30.0, 40.0, 50.0
- `LLV_Vendor_Recipes_Mods_PowerArmor_TSeries` (0x00437559) — Minimum Levels: 25.0, 30.0, 40.0
- `MTNS01_LL_Weapon_Instruments` (0x0043799F) — Minimum Levels: 1.0, 20.0
- `ENz01_Above_Quest_Rewards` (0x00438F22) — Minimum Levels: 1.0, 15.0
- `QuestReward_LLS_Ammo_All` (0x0043934C) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0
- `QuestReward_LLS_Aid_All` (0x0043934D) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0
- `QuestReward_LLS_AllRegions_Any` (0x0043B776) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0, 40.0, 45.0
- `QuestReward_LLS_AllRegions_Ammo` (0x0043B777) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0, 40.0, 45.0
- `QuestReward_LLS_AllRegions_Aid` (0x0043B778) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0, 40.0, 45.0
- `QuestReward_LLS_AllRegions_GrabBag` (0x0043B779) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0, 40.0, 45.0
- `QuestReward_LLS_AllRegions_Components_Assorted` (0x0043BA3F) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0, 40.0, 45.0
- `QuestReward_LLS_AllRegions_Legendary` (0x0043BA40) — Minimum Levels: 1.0, 10.0, 15.0, 25.0, 30.0, 35.0, 40.0, 45.0
- `Container_Loot_Priority_Trunk_Industrial_Boss` (0x00452A5D) — Minimum Levels: 1.0, 25.0
- `LDI_WaterPipe_FFZ17` (0x00452AA6) — Minimum Levels: 1.0, 15.0, 30.0, 45.0
- `LLD_Creature_Robot_Robobrain` (0x0045EC2C) — Minimum Levels: 1.0, 40.0
- `LLV_Vendor_Recipes_Base_Faction_BoS` (0x004958DB) — Minimum Levels: 1.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0
- `LLV_Vendor_Recipes_Base_Faction_FreeStates` (0x004958DC) — Minimum Levels: 1.0, 15.0, 20.0, 25.0, 30.0, 40.0
- `LLV_Vendor_Recipes_Base_Faction_Raiders` (0x004958DD) — Minimum Levels: 1.0, 5.0, 10.0, 15.0, 20.0, 25.0, 40.0
- `LLV_Vendor_Recipes_Base_Faction_Responders` (0x004958DE) — Minimum Levels: 1.0, 5.0, 10.0, 15.0, 25.0, 30.0
- `EN02_LL_MidQuestRewardItem` (0x004DF662) — Minimum Levels: 1.0, 25.0
- `LC060_Vendor_Whitespring_WeaponsAntiqueArms` (0x004E0FEB) — Minimum Levels: 1.0, 10.0, 15.0
- `LPI_Weapon_Ranged_LaserGun_Pistol_SemiAuto` (0x004E33A8) — Minimum Levels: 1.0, 5.0
- `Container_Loot_Priority_Trunk_Prewar_Boss_POI` (0x004ECB7C) — Minimum Levels: 1.0, 25.0
- `Container_Loot_Priority_Trunk_Raider_Boss_POI` (0x004ECB7D) — Minimum Levels: 1.0, 20.0
- `LPI_Weapon_Melee_Spear` (0x004ED7C3) — Minimum Levels: 1.0, 15.0
- `LLS_Creature_Scorched_Melee` (0x004EE6C9) — Minimum Levels: 1.0, 16.0, 22.0, 28.0, 36.0, 42.0, 48.0, 58.0, 68.0
- `LLS_MoleMiner_Melee` (0x004EE6CC) — Minimum Levels: 1.0, 4.0, 8.0, 14.0, 22.0, 30.0, 40.0
- `FFZ11_LL_QuestReward_Event` (0x00519A31) — Minimum Levels: 1.0, 95.0
- `LPI_Weapon_Ranged_Crossbow` (0x0052FEDD) — Minimum Levels: 1.0, 15.0
- `LLS_Creature_RadToad` (0x0056093B) — Minimum Levels: 1.0, 18.0, 28.0, 40.0
- `LL_Weapon_Ranged_Any` (0x00572CD0) — Minimum Levels: 1.0, 5.0, 15.0, 20.0, 25.0, 35.0
- `LL_Outfit_BloodEagle_Normal` (0x0058AFD5) — Minimum Levels: 1.0, 9.0, 20.0
- `LLI_Outfit_Cultist_Normal` (0x0058AFD9) — Minimum Levels: 1.0, 9.0, 20.0
- `LLI_Settler_Grenade_Frag_15` (0x0058D8C9) — Minimum Levels: 1.0, 14.0
- `LLI_Outfit_Settler_Guard` (0x0058E6C4) — Minimum Levels: 1.0, 20.0
- `CUT_LLI_Outfit_Raider_Radical` (0x0058E9DF) — Minimum Levels: 1.0, 9.0, 20.0
- `W05_LL_Armor_PowerArmor_Raider_Partial` (0x0058F270) — Minimum Levels: 1.0, 25.0, 30.0
- `LL_Outfit_Raider_Normal` (0x0058F304) — Minimum Levels: 1.0, 9.0, 20.0
- `LL_Outfit_Settler_Normal` (0x0058F322) — Minimum Levels: 1.0, 9.0, 20.0
- `WL020_LPI_CageWeaponReward` (0x00592349) — Minimum Levels: 1.0, 45.0, 55.0
- `W05_LLV_COMP_VisitorVendor` (0x00596746) — Minimum Levels: 1.0, 20.0
- `W05_LLV_COMP_VisitorVendor_Athena` (0x00596E8E) — Minimum Levels: 1.0, 20.0
- `W05_LLV_COMP_VisitorVendor_Emerson` (0x00596E8F) — Minimum Levels: 1.0, 20.0
- `W05_LLV_COMP_VisitorVendor_Sage` (0x00596E91) — Minimum Levels: 1.0, 20.0
- `W05_WST_WorkshopProduce_AmmoConstruction_Explosive` (0x00599475) — Minimum Levels: 20.0, 40.0
- `W05_LLV_Vendor_RE_CampAF05_Merchant_Armor` (0x0059A12C) — Minimum Levels: 1.0, 15.0
- `W05_LLV_Vendor_RE_CampAF09_Merchant_Weapon` (0x0059A132) — Minimum Levels: 1.0, 15.0
- `LL_Outfit_Settler_Guard_Normal` (0x0059F231) — Minimum Levels: 1.0, 20.0
- `LL_Outfit_RaiderCrater_Normal` (0x005A2F73) — Minimum Levels: 1.0, 9.0, 20.0
- `LLD_Creature_Catalyst_DungeonRahmani` (0x00612368) — Minimum Levels: 1.0, 2.0
- `LDI_MeatBag_E08B` (0x006431CC) — Minimum Levels: 1.0, 15.0, 25.0, 45.0
- `LDI_MeatBagFloor_E08B` (0x006431CD) — Minimum Levels: 1.0, 15.0, 25.0, 45.0
- `LDI_MeatBagHanging_E08B` (0x006464CE) — Minimum Levels: 1.0, 15.0, 30.0, 45.0
- `LL_Outfit_Refugee_Normal` (0x00654C50) — Minimum Levels: 1.0, 9.0, 20.0
- `LL_Outfit_Union_Normal` (0x006670AF) — Minimum Levels: 1.0, 9.0, 20.0
- `ATX_LL_COMP_Vendor_NukaAgent` (0x0067C2AE) — Minimum Levels: 1.0, 10.0
- `LPI_Weapon_Ranged_ThirstZapper` (0x006873BF) — Minimum Levels: 1.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0
- `LLI_Loot_Corpse_Weapons_CivilianCompetitor_XPD_AC_NONPLAYABLE` (0x006CDEC1) — Minimum Levels: 1.0, 5.0, 7.0, 14.0, 22.0, 28.0
- `LLI_Loot_Corpse_Weapons_Mobster_AC_NONPLAYABLE` (0x006CDEC2) — Minimum Levels: 1.0, 5.0, 7.0, 14.0, 22.0, 28.0
- `LLI_Loot_Corpse_Weapons_MunicipalAuditor_XPD_AC_NONPLAYABLE` (0x006CFEEB) — Minimum Levels: 1.0, 5.0, 7.0, 14.0, 22.0, 28.0
- `LLI_Loot_Corpse_Weapons_Showmen_XPD_AC_NONPLAYABLE` (0x006D1B8B) — Minimum Levels: 1.0, 5.0, 7.0, 14.0, 22.0, 28.0
- `LLS_Creature_RadTurkey` (0x0075469F) — Minimum Levels: 1.0, 22.0, 34.0, 46.0
- `LLD_AC_SQ03_SaltwaterSam544` (0x00755E95) — Minimum Levels: 1.0, 2.0
- `LL_WeaponUser_Junk_All` (0x007B7931) — Minimum Levels: 1.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0
- `crLLI_Burn_BountyHunt_Grenade_Energy` (0x008039A5) — Minimum Levels: 10.0, 15.0
- `LLI_BURN_RustRaider_GrenadierGrenades_50` (0x008325F2) — Minimum Levels: 1.0, 14.0
- `Burn_LL_GunVendor_Weapon_Boss_Thrown_Any` (0x0084C562) — Minimum Levels: 11.0, 15.0, 17.0, 22.0, 24.0, 26.0
- `Burn_LL_GunVendor_Weapon_Boss_Ranged_Any` (0x0084C567) — Minimum Levels: 1.0, 5.0
- `Burn_LL_GunVendor_Weapon_Boss_Melee_Any` (0x0084C56D) — Minimum Levels: 11.0, 15.0, 17.0, 22.0, 24.0, 26.0
- `LLD_Creature_Scorchbeast_PartyCrasher` (0x0089740A) — Minimum Levels: 1.0, 30.0, 50.0

## Rule D — overlapping-gate reward ladder (no Use All / Use First)

Two or more entries carry a one-sided range Condition (>=, >, <=, or < with no complementary bound in the same entry) and neither `Use All` nor `Use First Match` is set. Under the confirmed no-flag algorithm the engine builds the pool of entries whose Conditions currently pass and picks ONE uniformly at random — since eligibility isn't mutually exclusive, entries can overlap on a given roll instead of partitioning it the way an ordered rarity ladder usually implies. Not a confirmed bug: some hits are shared-threshold alternate pairs, or level/need-gated variety pools where overlap is the intended mechanic — `same-function` marks the shape most likely to be a mistake (every gated entry uses the identical Condition function, the classic hand-authored tier-ladder look). Odds are computed exactly only when every gate is `GetRandomPercent` with a resolved threshold and the entry count is <= 16.

### `LPI_Food_Prepared` (0x003A81B3) — same-function ladder
- entry 0 (`LL_Food_Prepared_Rare`) — GetRandomPercent Less Than Or Equal To LPI_Chance_Food_Prepared (=50) — naive Use-First read 50.0%, actual pool odds 37.5%
- entry 1 (`LL_Food_Prepared_Generic`) — GetRandomPercent Less Than Or Equal To LPI_Chance_Food_Prepared (=50) — naive Use-First read 25.0%, actual pool odds 37.5%
### `Test_LPI_FloraFern01` (0x003E0CB5) — mixed-function overlap
- entry 0 (`FloraRadFlashFern01`) — GetRandomPercent Less Than Or Equal To NukeFlora_SwapPercent_General_ECON (=10)
- entry 1 (`UseLPI_FloraFern01`) — GetLevel Greater Than 20.0
### `LLS_Festive_Rewards_Currency_Scrip` (0x0059CAC4) — same-function ladder
- entry 0 (`LegendaryTokens`) — GetRandomPercent Less Than Or Equal To 10.0 — naive Use-First read 10.0%, actual pool odds 3.8%
- entry 1 (`LegendaryTokens`) — GetRandomPercent Less Than Or Equal To 30.0 — naive Use-First read 27.0%, actual pool odds 12.1%
- entry 2 (`LegendaryTokens`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 31.5%, actual pool odds 21.8%
- entry 3 (`LegendaryTokens`) — (unconditioned) — naive Use-First read 31.5%, actual pool odds 62.3%
### `LLS_Festive_Rewards_Currency_Caps` (0x0059CAC5) — same-function ladder
- entry 0 (`Caps001`) — GetRandomPercent Less Than Or Equal To 5.0 — naive Use-First read 5.0%, actual pool odds 1.7%
- entry 1 (`Caps001`) — GetRandomPercent Less Than Or Equal To 20.0 — naive Use-First read 19.0%, actual pool odds 7.2%
- entry 2 (`Caps001`) — GetRandomPercent Less Than Or Equal To 80.0 — naive Use-First read 60.8%, actual pool odds 36.7%
- entry 3 (`Caps001`) — (unconditioned) — naive Use-First read 15.2%, actual pool odds 54.3%
### `LLS_Systemic_Rewards_Weapons_Plans` (0x0059CAF4) — same-function ladder
- entry 0 (`Recipe_Tinkers_GrenadePlasma`) — (unconditioned)
- entry 1 (`Recipe_Tinkers_MinePlasma`) — (unconditioned)
- entry 2 (`recipe_mod_melee_DeathclawGauntlet_Hook`) — GetLevel Greater Than Or Equal To 30.0
- entry 3 (`Recipe_Weapon_Melee_GrognaksAxe`) — GetLevel Greater Than Or Equal To 30.0
- entry 4 (`Recipe_Weapon_Thrown_Tomahawk`) — (unconditioned)
- entry 5 (`Recipe_Weapon_Melee_PoleHook`) — GetLevel Greater Than Or Equal To 10.0
- entry 6 (`Recipe_Weapon_Melee_LeadPipe`) — GetLevel Greater Than Or Equal To 1.0
- entry 7 (`Recipe_Weapon_Melee_GuitarSword`) — GetLevel Greater Than Or Equal To 15.0
- entry 8 (`Recipe_Weapon_Melee_MoleMinerGauntlet`) — GetLevel Greater Than Or Equal To 20.0
- entry 9 (`Recipe_Weapon_Ranged_Broadsider`) — GetLevel Greater Than Or Equal To 25.0
- entry 10 (`Recipe_Weapon_Mod_Ranged_LaserGun_SCOPE_MediumScope_Base`) — GetLevel Greater Than Or Equal To 5.0
- entry 11 (`Recipe_Weapon_Mod_Ranged_PipeGun_receiver_automatic_base`) — GetLevel Greater Than Or Equal To 1.0
- entry 12 (`Recipe_Weapon_Ranged_Ultracite_GatlingLaser`) — GetLevel Greater Than Or Equal To 35.0
- entry 13 (`Recipe_Weapon_Melee_BaseballBat`) — GetLevel Greater Than Or Equal To 1.0
- entry 14 (`Recipe_Weapon_Melee_Baton`) — GetLevel Greater Than Or Equal To 5.0
- entry 15 (`Recipe_Weapon_Ranged_Fatman`) — GetLevel Greater Than Or Equal To 25.0
- entry 16 (`Recipe_Weapon_Ranged_GammaGun`) — GetLevel Greater Than Or Equal To 15.0
- entry 17 (`Recipe_Weapon_Ranged_LaserGun`) — GetLevel Greater Than Or Equal To 5.0
- entry 18 (`Recipe_Weapon_Melee_BoxingGlove`) — GetLevel Greater Than Or Equal To 5.0
- entry 19 (`Recipe_Weapon_Ranged_HarpoonGun`) — GetLevel Greater Than Or Equal To 30.0
- entry 20 (`Recipe_Weapon_Ranged_PumpActionShotgun`) — GetLevel Greater Than Or Equal To 5.0
- entry 21 (`recipe_mod_melee_DeathclawGauntlet_Hook`) — GetLevel Greater Than Or Equal To 30.0
### `LLS_Systemic_Rewards_Armor_Plans` (0x0059CAF5) — same-function ladder
- entry 0 (`Recipe_Armor_Metal_Torso_Heavy`) — GetLevel Greater Than Or Equal To 10.0
- entry 1 (`Recipe_Armor_Metal_Torso_Medium`) — GetLevel Greater Than Or Equal To 10.0
- entry 2 (`Recipe_Armor_Metal_Legs_Medium`) — GetLevel Greater Than Or Equal To 10.0
- entry 3 (`Recipe_Armor_Metal_Legs_Heavy`) — GetLevel Greater Than Or Equal To 10.0
- entry 4 (`Recipe_Armor_Metal_Arms_Heavy`) — GetLevel Greater Than Or Equal To 10.0
- entry 5 (`Recipe_Armor_Metal_Arms_Medium`) — GetLevel Greater Than Or Equal To 10.0
- entry 6 (`Recipe_Armor_Metal_Torso_Light`) — GetLevel Greater Than Or Equal To 10.0
- entry 7 (`Recipe_Armor_Metal_Arms_Light`) — GetLevel Greater Than Or Equal To 10.0
- entry 8 (`Recipe_Armor_Metal_Legs_Light`) — GetLevel Greater Than Or Equal To 10.0
- entry 9 (`Recipe_Armor_Robot_Torso_Heavy`) — GetLevel Greater Than Or Equal To 10.0
- entry 10 (`Recipe_Armor_Robot_Torso_Medium`) — GetLevel Greater Than Or Equal To 10.0
- entry 11 (`Recipe_Armor_Robot_Arms_Medium`) — GetLevel Greater Than Or Equal To 10.0
- entry 12 (`Recipe_Armor_Robot_Legs_Heavy`) — GetLevel Greater Than Or Equal To 10.0
- entry 13 (`Recipe_Armor_Robot_Legs_Medium`) — GetLevel Greater Than Or Equal To 10.0
- entry 14 (`Recipe_Armor_Robot_Arms_Heavy`) — GetLevel Greater Than Or Equal To 10.0
- entry 15 (`Recipe_Armor_Robot_Torso_Light`) — GetLevel Greater Than Or Equal To 10.0
- entry 16 (`Recipe_Armor_Robot_Arms_Light`) — GetLevel Greater Than Or Equal To 10.0
- entry 17 (`Recipe_Armor_Robot_Legs_Light`) — GetLevel Greater Than Or Equal To 10.0
- entry 18 (`Recipe_Armor_Combat_Torso_Heavy`) — GetLevel Greater Than Or Equal To 20.0
- entry 19 (`Recipe_Armor_Combat_Torso_Medium`) — GetLevel Greater Than Or Equal To 20.0
- entry 20 (`Recipe_Armor_Combat_Arms_Heavy`) — GetLevel Greater Than Or Equal To 20.0
- entry 21 (`Recipe_Armor_Combat_Legs_Heavy`) — GetLevel Greater Than Or Equal To 20.0
- entry 22 (`Recipe_Armor_Combat_Legs_Medium`) — GetLevel Greater Than Or Equal To 20.0
- entry 23 (`Recipe_Armor_Combat_Arms_Medium`) — GetLevel Greater Than Or Equal To 20.0
- entry 24 (`Recipe_Armor_Combat_Torso_Light`) — GetLevel Greater Than Or Equal To 20.0
- entry 25 (`Recipe_Armor_Combat_Arms_Light`) — GetLevel Greater Than Or Equal To 20.0
- entry 26 (`Recipe_Armor_Combat_Legs_Light`) — GetLevel Greater Than Or Equal To 20.0
- entry 27 (`Recipe_Armor_Raider_Torso_Heavy`) — GetLevel Greater Than Or Equal To 5.0
- entry 28 (`Recipe_Armor_Raider_Torso_Light`) — GetLevel Greater Than Or Equal To 5.0
- entry 29 (`Recipe_Armor_Raider_Legs_Heavy`) — GetLevel Greater Than Or Equal To 5.0
- entry 30 (`Recipe_Armor_Raider_Arms_Heavy`) — GetLevel Greater Than Or Equal To 5.0
- entry 31 (`Recipe_Armor_Raider_Legs_Light`) — GetLevel Greater Than Or Equal To 5.0
- entry 32 (`Recipe_Armor_Raider_Arms_Light`) — GetLevel Greater Than Or Equal To 5.0
- entry 33 (`Recipe_Armor_Raider_Torso_Medium`) — GetLevel Greater Than Or Equal To 5.0
- entry 34 (`Recipe_Armor_Raider_Arms_Medium`) — GetLevel Greater Than Or Equal To 5.0
- entry 35 (`Recipe_Armor_Raider_Legs_Medium`) — GetLevel Greater Than Or Equal To 5.0
### `LLS_Systemic_Rewards_Armor_Mods` (0x0059CAF6) — same-function ladder
- entry 0 (`recipe_mod_armor_Combat_Lining_Torso_Explosion2`) — GetLevel Greater Than Or Equal To 20.0
- entry 1 (`recipe_mod_armor_Leather_Lining_Torso_Explosion2`) — GetLevel Greater Than Or Equal To 1.0
- entry 2 (`recipe_mod_armor_RaiderMod_Lining_Torso_Explosion2`) — GetLevel Greater Than Or Equal To 5.0
- entry 3 (`recipe_mod_armor_Robot_Lining_Torso_Explosion2`) — GetLevel Greater Than Or Equal To 10.0
- entry 4 (`recipe_mod_armor_Metal_Lining_Torso_Explosion2`) — GetLevel Greater Than Or Equal To 10.0
- entry 5 (`recipe_mod_armor_Robot_Lining_Torso_ImprovedCarryCapacity`) — GetLevel Greater Than Or Equal To 10.0
- entry 6 (`recipe_mod_armor_Robot_Lining_Limb_ImprovedCarryCapacity`) — GetLevel Greater Than Or Equal To 10.0
- entry 7 (`recipe_mod_armor_RaiderMod_Lining_Limb_ImprovedCarryCapacity`) — GetLevel Greater Than Or Equal To 5.0
- entry 8 (`recipe_mod_armor_RaiderMod_Lining_Torso_ImprovedCarryCapacity`) — GetLevel Greater Than Or Equal To 5.0
- entry 9 (`recipe_mod_armor_Trapper_Lining_Limb_ImprovedCarryCapacity`) — GetLevel Greater Than Or Equal To 15.0
- entry 10 (`recipe_mod_armor_Trapper_Lining_Torso_ImprovedCarryCapacity`) — GetLevel Greater Than Or Equal To 15.0
- entry 11 (`recipe_mod_armor_Trapper_Lining_Torso_Lighter2`) — GetLevel Greater Than Or Equal To 15.0
- entry 12 (`recipe_mod_armor_Trapper_Lining_Limb_Lighter2`) — GetLevel Greater Than Or Equal To 15.0
- entry 13 (`recipe_mod_armor_RaiderMod_Lining_Torso_Lighter2`) — GetLevel Greater Than Or Equal To 5.0
- entry 14 (`recipe_mod_armor_RaiderMod_Lining_Limb_Lighter2`) — GetLevel Greater Than Or Equal To 5.0
- entry 15 (`recipe_mod_armor_Combat_Lining_Limb_Lighter2`) — GetLevel Greater Than Or Equal To 10.0
- entry 16 (`recipe_mod_armor_Robot_Lining_Torso_Lighter2`) — GetLevel Greater Than Or Equal To 10.0
- entry 17 (`recipe_mod_armor_Leather_Lining_Limb_Lighter2`) — GetLevel Greater Than Or Equal To 1.0
- entry 18 (`recipe_mod_armor_Leather_Lining_Torso_Lighter2`) — GetLevel Greater Than Or Equal To 1.0
- entry 19 (`recipe_mod_armor_Robot_Lining_Limb_Lighter2`) — GetLevel Greater Than Or Equal To 10.0
- entry 20 (`recipe_mod_armor_Metal_Lining_Torso_Lighter2`) — GetLevel Greater Than Or Equal To 10.0
- entry 21 (`recipe_mod_armor_Metal_Lining_Limb_Lighter2`) — GetLevel Greater Than Or Equal To 10.0
### `LLS_Systemic_Rewards_PowerArmor_Plans` (0x0059CAF7) — same-function ladder
- entry 0 (`recipe_Armor_PowerArmor_T45_ArmLeft`) — GetLevel Greater Than Or Equal To 25.0
- entry 1 (`recipe_Armor_PowerArmor_T45_ArmRight`) — GetLevel Greater Than Or Equal To 25.0
- entry 2 (`recipe_Armor_PowerArmor_T45_Helmet`) — GetLevel Greater Than Or Equal To 25.0
- entry 3 (`recipe_Armor_PowerArmor_T45_LegLeft`) — GetLevel Greater Than Or Equal To 25.0
- entry 4 (`recipe_Armor_PowerArmor_T45_LegRight`) — GetLevel Greater Than Or Equal To 25.0
- entry 5 (`recipe_Armor_PowerArmor_T45_Torso`) — GetLevel Greater Than Or Equal To 25.0
- entry 6 (`recipe_Armor_PowerArmor_Raider_ArmLeft`) — GetLevel Greater Than Or Equal To 15.0
- entry 7 (`recipe_Armor_PowerArmor_Raider_ArmRight`) — GetLevel Greater Than Or Equal To 15.0
- entry 8 (`recipe_Armor_PowerArmor_Raider_Helmet`) — GetLevel Greater Than Or Equal To 15.0
- entry 9 (`recipe_Armor_PowerArmor_Raider_LegLeft`) — GetLevel Greater Than Or Equal To 15.0
- entry 10 (`recipe_Armor_PowerArmor_Raider_LegRight`) — GetLevel Greater Than Or Equal To 15.0
- entry 11 (`recipe_Armor_PowerArmor_Raider_Torso`) — GetLevel Greater Than Or Equal To 15.0
- entry 12 (`recipe_Armor_PowerArmor_T51_ArmLeft`) — GetLevel Greater Than Or Equal To 30.0
- entry 13 (`recipe_Armor_PowerArmor_T51_ArmRight`) — GetLevel Greater Than Or Equal To 30.0
- entry 14 (`recipe_Armor_PowerArmor_T51_Helmet`) — GetLevel Greater Than Or Equal To 30.0
- entry 15 (`recipe_Armor_PowerArmor_T51_LegLeft`) — GetLevel Greater Than Or Equal To 30.0
- entry 16 (`recipe_Armor_PowerArmor_T51_LegRight`) — GetLevel Greater Than Or Equal To 30.0
- entry 17 (`recipe_Armor_PowerArmor_T51_Torso`) — GetLevel Greater Than Or Equal To 30.0
### `LLS_Systemic_Rewards_PowerArmor_Mods` (0x0059CAF8) — same-function ladder
- entry 0 (`LLS_Recipes_Mods_PowerArmor_Raider_Tier1`) — GetLevel Greater Than Or Equal To 15.0
- entry 1 (`LLS_Recipes_Mods_PowerArmor_Raider_Tier2`) — GetLevel Greater Than Or Equal To 15.0
- entry 2 (`LLS_Recipes_Mods_PowerArmor_Raider_Tier3`) — GetLevel Greater Than Or Equal To 15.0
- entry 3 (`LLS_Recipes_Mods_PowerArmor_T45_Tier1`) — GetLevel Greater Than Or Equal To 25.0
- entry 4 (`LLS_Recipes_Mods_PowerArmor_T45_Tier2`) — GetLevel Greater Than Or Equal To 25.0
- entry 5 (`LLS_Recipes_Mods_PowerArmor_T45_Tier3`) — GetLevel Greater Than Or Equal To 25.0
- entry 6 (`LLS_Recipes_Mods_PowerArmor_T51_Tier1`) — GetLevel Greater Than Or Equal To 30.0
- entry 7 (`LLS_Recipes_Mods_PowerArmor_T51_Tier2`) — GetLevel Greater Than Or Equal To 30.0
- entry 8 (`LLS_Recipes_Mods_PowerArmor_T51_Tier3`) — GetLevel Greater Than Or Equal To 30.0
### `LLS_Creature_WorldBoss_Currency_Caps` (0x005A405D) — same-function ladder
- entry 0 (`Caps001`) — GetRandomPercent Less Than Or Equal To 20.0 — naive Use-First read 20.0%, actual pool odds 6.5%
- entry 1 (`Caps001`) — GetRandomPercent Less Than Or Equal To 40.0 — naive Use-First read 32.0%, actual pool odds 13.9%
- entry 2 (`Caps001`) — GetRandomPercent Less Than Or Equal To 80.0 — naive Use-First read 38.4%, actual pool odds 32.5%
- entry 3 (`Caps001`) — (unconditioned) — naive Use-First read 9.6%, actual pool odds 47.1%
### `LLS_Generic_Rewards_Currency_Caps_25-500` (0x005A70BE) — same-function ladder
- entry 0 (`Caps001`) — GetRandomPercent Less Than Or Equal To 5.0 — naive Use-First read 5.0%, actual pool odds 1.6%
- entry 1 (`Caps001`) — GetRandomPercent Less Than Or Equal To 40.0 — naive Use-First read 38.0%, actual pool odds 14.6%
- entry 2 (`Caps001`) — GetRandomPercent Less Than Or Equal To 80.0 — naive Use-First read 45.6%, actual pool odds 34.4%
- entry 3 (`Caps001`) — GetRandomPercent Less Than Or Equal To 99.0 — naive Use-First read 11.3%, actual pool odds 49.3%
### `ATX_Resources_Collectron_Gold_Scrap` (0x005F0D28) — same-function ladder
- entry 0 (`ATX_Resources_Collectron_Gold`) — GetRandomPercent Greater Than Or Equal To 0.95 — naive Use-First read 99.0%, actual pool odds 24.9%
- entry 1 (`ATX_Resources_Collectron_Scrap_Uncommon`) — GetRandomPercent Greater Than Or Equal To 0.82 — naive Use-First read 0.9%, actual pool odds 24.9%
- entry 2 (`ATX_Resources_Collectron_Scrap_Common`) — GetRandomPercent Greater Than Or Equal To 0.65 — naive Use-First read 0.0%, actual pool odds 25.0%
- entry 3 (`ATX_Resources_Collectron_Scrap_SuperCommon`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 25.2%
### `RESTRICTED_LL_LegendaryModule_2-4` (0x006135A2) — same-function ladder
- entry 0 (`LegendaryModule`) — GetRandomPercent Less Than Or Equal To 20.0 — naive Use-First read 20.0%, actual pool odds 8.7%
- entry 1 (`LegendaryModule`) — GetRandomPercent Less Than Or Equal To 40.0 — naive Use-First read 32.0%, actual pool odds 18.7%
- entry 2 (`LegendaryModule`) — (unconditioned) — naive Use-First read 48.0%, actual pool odds 72.7%
### `ATX_Resources_Collectron_Silver_Scrap` (0x0064A14A) — same-function ladder
- entry 0 (`ATX_Resources_Collectron_Silver`) — GetRandomPercent Greater Than Or Equal To 0.95 — naive Use-First read 99.0%, actual pool odds 24.9%
- entry 1 (`ATX_Resources_Collectron_Scrap_Uncommon`) — GetRandomPercent Greater Than Or Equal To 0.82 — naive Use-First read 0.9%, actual pool odds 24.9%
- entry 2 (`ATX_Resources_Collectron_Scrap_Common`) — GetRandomPercent Greater Than Or Equal To 0.65 — naive Use-First read 0.0%, actual pool odds 25.0%
- entry 3 (`ATX_Resources_Collectron_Scrap_SuperCommon`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 25.2%
### `E09D_Weapons` (0x00668F1D) — same-function ladder
- entry 0 (`LL_Weapon_Ranged_SingleActionRevolver_GunthersRevolver`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 50.0%, actual pool odds 37.5%
- entry 1 (`LL_Weapon_Ranged_LeverGun_WesternSpirit`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 25.0%, actual pool odds 37.5%
### `ATX_LL_NukaColaMysteryMachine` (0x0067A293) — same-function ladder
- entry 0 (`ATX_LL_NukaColaMysteryMachine_Rare`) — GetRandomPercent Greater Than Or Equal To 90.0 — naive Use-First read 10.0%, actual pool odds 4.5%
- entry 1 (`ATX_LL_NukaColaMysteryMachine_Uncommon`) — GetRandomPercent Greater Than Or Equal To 70.0 — naive Use-First read 27.0%, actual pool odds 14.5%
- entry 2 (`ATX_LL_NukaColaMysteryMachine_Common`) — (unconditioned) — naive Use-First read 63.0%, actual pool odds 81.0%
### `MOON_LL_TreasuryNotes` (0x006B4187) — same-function ladder
- entry 0 (`Treasury_Note`) — (unconditioned) — naive Use-First read 100.0%, actual pool odds 61.0%
- entry 1 (`Treasury_Note`) — GetRandomPercent Greater Than 40.0 — naive Use-First read 0.0%, actual pool odds 27.0%
- entry 2 (`Treasury_Note`) — GetRandomPercent Greater Than 70.0 — naive Use-First read 0.0%, actual pool odds 12.0%
### `RNG_Storm_Flora_Firecap` (0x006D9C59) — same-function ladder
- entry 0 (`FloraRadFireCap01`) — GetRandomPercent Less Than Or Equal To NukeFlora_SwapPercent_Red_ECON (=100) — naive Use-First read 100.0%, actual pool odds 39.2%
- entry 1 (`UseLPI_FloraFireCap01`) — GetRandomPercent Less Than Or Equal To LPI_Chance_Flora_FireCap (=65) — naive Use-First read 0.0%, actual pool odds 21.7%
- entry 2 (`UseLPI_FloraFireCap01_Harvested`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 39.2%
### `LL_WeaponUser_Junk_Bones` (0x007B31D8) — same-function ladder
- entry 0 (`BonesFemur`) — (unconditioned)
- entry 1 (`BonesFemurSnapped01`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 2 (`BonesFemurSnapped02`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 3 (`BonesHandLeft`) — (unconditioned)
- entry 4 (`BonesHandRight`) — (unconditioned)
- entry 5 (`BonesLeftArm`) — (unconditioned)
- entry 6 (`BonesLeftFoot`) — (unconditioned)
- entry 7 (`BonesLeftLeg`) — (unconditioned)
- entry 8 (`BonesPelvis`) — (unconditioned)
- entry 9 (`BonesRibCage`) — (unconditioned)
- entry 10 (`BonesRibCage02`) — (unconditioned)
- entry 11 (`BonesRibCagePelvis`) — (unconditioned)
- entry 12 (`BonesRightArm`) — (unconditioned)
- entry 13 (`BonesRightFoot`) — (unconditioned)
- entry 14 (`BonesRightLeg`) — (unconditioned)
- entry 15 (`BonesSkull`) — (unconditioned)
- entry 16 (`BonesSkullFragments01`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 17 (`BonesSkullFragments02`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 18 (`BonesSkullFragments03`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 19 (`BonesSkullFragments04`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 20 (`BonesSkullFragments05`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 21 (`BonesSkullFragments06`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 22 (`BonesSkullUpper`) — (unconditioned)
- entry 23 (`BonesSpine`) — (unconditioned)
- entry 24 (`BonesTibia`) — (unconditioned)
### `LL_WeaponUser_Junk_Plastic` (0x007B31E8) — same-function ladder
- entry 0 (`BloodPack_Empty`) — GetRandomPercent Less Than Or Equal To 25.0 — naive Use-First read 25.0%, actual pool odds 2.8%
- entry 1 (`CafeteriaTray`) — (unconditioned) — naive Use-First read 75.0%, actual pool odds 12.3%
- entry 2 (`CatBowl`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 12.3%
- entry 3 (`Coolant_Empty01`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 12.3%
- entry 4 (`DogBowl`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 12.3%
- entry 5 (`Doll_Arm`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 12.3%
- entry 6 (`Hairbrush_01`) — GetRandomPercent Less Than Or Equal To 25.0 — naive Use-First read 0.0%, actual pool odds 2.8%
- entry 7 (`Knife_01_Plastic`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 12.3%
- entry 8 (`Pen01`) — GetRandomPercent Less Than Or Equal To 25.0 — naive Use-First read 0.0%, actual pool odds 2.8%
- entry 9 (`Spoon_01_Plastic`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 12.3%
- entry 10 (`Toothbrush`) — GetRandomPercent Less Than Or Equal To 25.0 — naive Use-First read 0.0%, actual pool odds 2.8%
- entry 11 (`Toothpaste`) — GetRandomPercent Less Than Or Equal To 25.0 — naive Use-First read 0.0%, actual pool odds 2.8%
### `LL_WeaponUser_Junk_Screws` (0x007B31E9) — same-function ladder
- entry 0 (`Handcuffs`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 50.0%, actual pool odds 9.9%
- entry 1 (`ToyCar`) — (unconditioned) — naive Use-First read 50.0%, actual pool odds 22.4%
- entry 2 (`ToyTruck01`) — GetRandomPercent Less Than Or Equal To 75.0 — naive Use-First read 0.0%, actual pool odds 15.7%
- entry 3 (`c_Screws_scrap`) — (unconditioned) — naive Use-First read 0.0%, actual pool odds 22.4%
- entry 4 (`PlayerHouse_Ruin_PepperMill01`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 0.0%, actual pool odds 9.9%
- entry 5 (`SilverLocket`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 0.0%, actual pool odds 9.9%
- entry 6 (`Clipboard_Prewar01_Clean`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 0.0%, actual pool odds 9.9%
### `LL_WeaponUser_Junk_Steel` (0x007B31EB) — same-function ladder
- entry 0 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 1 (`Bonesaw`) — GetRandomPercent Less Than Or Equal To 25.0
- entry 2 (`CoffeePot01`) — (unconditioned)
- entry 3 (`Colander`) — (unconditioned)
- entry 4 (`CookingPan01`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 5 (`CookingPot01`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 6 (`AutoPart04`) — (unconditioned)
- entry 7 (`EnamelBucket01`) — (unconditioned)
- entry 8 (`Hammer01`) — (unconditioned)
- entry 9 (`Handcuffs`) — (unconditioned)
- entry 10 (`Ladle`) — (unconditioned)
- entry 11 (`Lighter`) — (unconditioned)
- entry 12 (`OilCan01`) — (unconditioned)
- entry 13 (`PaintCanEmpty`) — GetRandomPercent Less Than Or Equal To 50.0
- entry 14 (`Plate_02_Dinner`) — (unconditioned)
- entry 15 (`Scalpel`) — (unconditioned)
- entry 16 (`Scissors`) — (unconditioned)
- entry 17 (`ScrewDriver01`) — (unconditioned)
- entry 18 (`ShoppingBasket`) — (unconditioned)
- entry 19 (`OilCan01`) — (unconditioned)
- entry 20 (`CookingPot01`) — (unconditioned)
- entry 21 (`c_Steel_scrap`) — (unconditioned)
- entry 22 (`TinCan01`) — (unconditioned)
- entry 23 (`TinCan01`) — (unconditioned)
- entry 24 (`TinCan01`) — (unconditioned)
- entry 25 (`TinCan03`) — (unconditioned)
- entry 26 (`TinCan03`) — (unconditioned)
- entry 27 (`TinCan03`) — (unconditioned)
- entry 28 (`ToyTruck01`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 29 (`Wrench01`) — (unconditioned)
- entry 30 (`Wrench02`) — (unconditioned)
- entry 31 (`Wrench03`) — (unconditioned)
- entry 32 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 33 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 34 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 35 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 36 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 37 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
- entry 38 (`c_Steel_scrap`) — GetRandomPercent Less Than Or Equal To 75.0
### `P62_LLS_Drifter_Rewards_LegendaryShards_Fallback` (0x00802171) — same-function ladder
- entry 0 (`P62_LegendaryItems_Drifter_Rank4`) — GetRandomPercent Less Than Or Equal To 25.0 — naive Use-First read 25.0%, actual pool odds 8.1%
- entry 1 (`P62_LegendaryItems_Drifter_Rank3`) — GetRandomPercent Less Than Or Equal To 50.0 — naive Use-First read 37.5%, actual pool odds 17.4%
- entry 2 (`P62_LegendaryItems_Drifter_Rank2`) — GetRandomPercent Less Than Or Equal To 75.0 — naive Use-First read 28.1%, actual pool odds 28.9%
- entry 3 (`P62_LegendaryItems_Drifter_Rank1`) — (unconditioned) — naive Use-First read 9.4%, actual pool odds 45.6%
### `SCORE_S22_Resources_Collector_SoulSoupServer_Food` (0x008308D7) — same-function ladder
- entry 0 (`BrainFungusVegetableCookedSoup`) — GetRandomPercent Greater Than Or Equal To 92.0 — naive Use-First read 8.0%, actual pool odds 2.2%
- entry 1 (`SiltBeanVegetableCookedSoup`) — GetRandomPercent Greater Than Or Equal To 80.0 — naive Use-First read 18.4%, actual pool odds 5.7%
- entry 2 (`SwampPlantTastyTofuSoup`) — GetRandomPercent Greater Than Or Equal To 63.0 — naive Use-First read 27.2%, actual pool odds 11.0%
- entry 3 (`PumpkinVegetableCookedSoup`) — GetRandomPercent Greater Than Or Equal To 45.0 — naive Use-First read 25.5%, actual pool odds 17.2%
- entry 4 (`CornVegetableCookedSoup`) — GetRandomPercent Greater Than Or Equal To 25.0 — naive Use-First read 15.6%, actual pool odds 25.2%
- entry 5 (`FirecapCookedSoup`) — (unconditioned) — naive Use-First read 5.2%, actual pool odds 38.8%
### `LL_ChemMysteryMachine` (0x00853D35) — same-function ladder
- entry 0 (`SCORE_S23_LL_ChemMysteryMachine_Common`) — (unconditioned) — naive Use-First read 100.0%, actual pool odds 72.7%
- entry 1 (`SCORE_S23_LL_ChemMysteryMachine_Uncommon`) — GetRandomPercent Greater Than Or Equal To 60.0 — naive Use-First read 0.0%, actual pool odds 18.7%
- entry 2 (`SCORE_S23_LL_ChemMysteryMachine_Rare`) — GetRandomPercent Greater Than Or Equal To 80.0 — naive Use-First read 0.0%, actual pool odds 8.7%
### `RA_LL_Rewards_General_AidItems_Rare_Need` (0x0086A8C7) — same-function ladder
- entry 0 (`BerryMentats`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 1 (`GrapeMentats`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 2 (`OrangeMentats`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 3 (`Bufftats`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 4 (`Psychobuff`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 5 (`Psychotats`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 6 (`DaddyO`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 7 (`DayTripper`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 8 (`Fury`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 9 (`Calmex`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 10 (`Overdrive`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 11 (`XCell`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 12 (`SuperStimpak`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 13 (`Addictol`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
### `RA_LL_Rewards_General_AidItems_Common_Need` (0x0086A8C8) — same-function ladder
- entry 0 (`Buffout`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 1 (`MedX`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 2 (`Mentats`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 3 (`Psycho`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 4 (`Stimpak`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 5 (`RadAway`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 6 (`RadX`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
- entry 7 (`SURV_DiseaseCure_HerbalMedicine`) — GetItemCount Less Than Or Equal To RA_ChemNeedsAmount (unresolved)
### `SDOW_MQ02_Graves_LL_QuestRelatedItems` (0x008F2AFD) — same-function ladder
- entry 0 (`SDOW_MQ02_Graves_LL_FirstJournalPage`) — GetValue Less Than Or Equal To 0.0
- entry 1 (`SDOW_MQ02_Graves_LL_JournalPages`) — GetValue Greater Than Or Equal To 1.0
